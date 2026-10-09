// AX's Node bridge for DSH-style register(on, options) modules.
import { createInterface } from 'node:readline';
import { promises as fs } from 'node:fs';
import path from 'node:path';
import { pathToFileURL } from 'node:url';
import { register } from 'node:module';

const sdk = `export const defineMod=spec=>({definition:{...spec,options:spec.userConfig??{}}});`;
const loader = `export async function resolve(s,c,n){if(['@ax/mods','@deepseek-ai/dsh-experimental-claude-code-mods'].includes(s))return {url:${JSON.stringify('data:text/javascript,'+encodeURIComponent(sdk))},shortCircuit:true};return n(s,c)}`;
register('data:text/javascript,'+encodeURIComponent(loader),import.meta.url);
const send = data => process.stdout.write(JSON.stringify(data)+'\n');
console.log = (...args) => console.error(...args);
const pending = new Map(), hooks = [], tools = new Map(), commands = new Map(), states = new Map(), timers = new Set();
let sequence=0,session={},home='',turns=0,started=0,messages=[],active=false;
const supported=new Set(['session.start','session.end','prompt.submit','turn.start','turn.complete','tool.call','command.run']);
const host = (call,input) => {
  if(!active) throw new Error('Host operations require an active Mod event');
  const callId=++sequence;
  return new Promise((resolve,reject)=>{pending.set(callId,{resolve,reject});send({call,callId,input})});
};
const timeout = async (promise,ms=10000) => {
  let timer;
  try {return await Promise.race([promise,new Promise((_,reject)=>{timer=setTimeout(()=>reject(new Error('Mod hook timed out')),ms)})]);}
  finally {clearTimeout(timer);}
};
const matches=(matcher,event)=>Object.entries(matcher??{}).every(([key,value])=>value instanceof RegExp?(value.lastIndex=0,value.test(String(event[key]??''))):Array.isArray(value)?value.includes(event[key]):value===event[key]);
const validName=name=>typeof name==='string'&&/^[\p{L}\p{N}_.-]+$/u.test(name)&&name!=='.'&&name!=='..';
const requireName=name=>{if(!validName(name))throw new Error('Invalid Mod registration name');};
const unsupported=namespace=>new Proxy({}, {get:(_,method)=>()=>{throw new Error(`AX Mod API does not implement ${namespace}.${String(method)}`)}});

function api(mod) {
  const key=reference=>`${reference?.plugin??mod.name}:${reference?.key??reference}`;
  const storeFile=()=>path.join(home,'mod-store',mod.name+'.json');
  const readStore=async()=>{try{return JSON.parse(await fs.readFile(storeFile(),'utf8'))}catch(error){if(error.code==='ENOENT')return {};throw error}};
  const saveStore=async store=>{const bytes=JSON.stringify(store);if(Buffer.byteLength(bytes)>4*1024*1024)throw new Error('Mod store exceeds 4 MiB');await fs.mkdir(path.dirname(storeFile()),{recursive:true});const temporary=storeFile()+'.'+process.pid+'.tmp';await fs.writeFile(temporary,bytes,{mode:0o600});await fs.rename(temporary,storeFile());};
  const textElement=type=>(props,...children)=>({type,props:props??{},children:children.flat(Infinity)});
  const state={get:async ref=>({value:states.get(key(ref))}),set:async(ref,value)=>{states.set(key(ref),value)},delete:async ref=>{states.delete(key(ref))}};
  const store={get:async ref=>({value:(await readStore())[key(ref)]}),set:async(ref,value)=>{const values=await readStore();values[key(ref)]=value;await saveStore(values)},delete:async ref=>{const values=await readStore();delete values[key(ref)];await saveStore(values)}};
  const command={register:async spec=>{requireName(spec.name);if(commands.has(spec.name))throw new Error('Duplicate Mod command: '+spec.name);commands.set(spec.name,{...spec,plugin:mod.name})},list:async()=>[...commands.values()],run:input=>dispatch('command.run',{...input,args:input.args??'',origin:{kind:'plugin',name:mod.name}},()=>{throw new Error('No Mod answered command '+input.command)})};
  const tool={register:async spec=>{requireName(spec.name);const name=`mcp__${mod.name}__${spec.name}`;if(tools.has(name))throw new Error('Duplicate Mod tool: '+name);tools.set(name,{...spec,name,plugin:mod.name,inputSchema:spec.inputSchema??{type:'object',properties:{}}})},list:async()=>[...tools.values()],call:input=>dispatch('tool.call',input,async e=>host('tool',e))};
  const checkedPath=async value=>{const resolved=path.resolve(session.cwd,value);const real=await fs.realpath(resolved);const base=await fs.realpath(session.cwd);if(real!==base&&!real.startsWith(base+path.sep))throw new Error('Mod path escapes workspace');return real};
  const ui={resolve:()=>({Box:textElement('Box'),Text:textElement('Text'),Button:textElement('Button')}),open:async()=>({isPlaced:false}),close:async()=>{},panes:async()=>[],invalidate:async()=>{},log:async input=>console.error('[Mod]',mod.name,input),toast:async input=>console.error('[Mod]',mod.name,input),status:async input=>console.error('[Mod]',mod.name,input),ask:async()=>{throw new Error('Mod ui.ask is unavailable; use request_user_input')}};
  const clock={now:async()=>Date.now(),sleep:ms=>new Promise(resolve=>setTimeout(resolve,ms)),after:async(ms,callback)=>{const timer=setTimeout(()=>{timers.delete(timer);Promise.resolve(callback()).catch(console.error)},ms);timers.add(timer);return timer},every:async(ms,callback)=>{const timer=setInterval(()=>Promise.resolve(callback()).catch(console.error),ms);timers.add(timer);return timer}};
  return new Proxy({plugin:{name:mod.name,root:mod.root,tier:'user'},state,store,command,tool,ui,clock,
    session:{id:async()=>session.id,cwd:async()=>session.cwd,root:async()=>session.cwd,model:async()=>session.model,turns:async()=>turns,messages:async()=>messages,version:async()=>({host:'AX'}),usage:async()=>({context:{window:session.window??0},rateLimits:[]})},
    fs:{read:async input=>{const file=await checkedPath(typeof input==='string'?input:input.path);if((await fs.stat(file)).size>4*1024*1024)throw new Error('Mod file exceeds 4 MiB');return {text:await fs.readFile(file,'utf8')}},exists:async input=>{try{await checkedPath(typeof input==='string'?input:input.path);return true}catch{return false}},list:async input=>fs.readdir(await checkedPath(typeof input==='string'?input:input.path)),stat:async input=>{const stat=await fs.stat(await checkedPath(typeof input==='string'?input:input.path));return {size:stat.size,isFile:stat.isFile(),isDirectory:stat.isDirectory(),mtimeMs:stat.mtimeMs}},write:input=>host('tool',{tool:'filesystem',operation:'write',path:input.path,content:input.text??input.content})},
    process:unsupported('process'),
    http:{fetch:input=>host('tool',{tool:'web',operation:'fetch',urls:[typeof input==='string'?input:input.url]})},
    env:{get:async name=>process.env[name],set:async(name,value)=>{process.env[name]=value}},prompt:{submit:async()=>{throw new Error('AX Mod prompt.submit is unavailable; use prompt.submit hook context')}}
  },{get:(target,name)=>name in target?target[name]:unsupported(String(name))});
}

async function dispatch(event,input,terminal=async e=>e) {
  const selected=hooks.filter(h=>h.event===event&&matches(h.matcher,input));
  const run=async(index,e)=>{
    if(index>=selected.length)return terminal(e);
    const h=selected[index];let once,called=false;
    const next=nextInput=>{called=true;return once??=run(index+1,nextInput??e)};
    Object.assign(next,{signal:new AbortController().signal,origin:{plugin:'AX',tier:'core'},budget:{ms:10000,remainingMs:10000},to:()=>{throw new Error('Managed Mod tiers are not supported')}});
    try {
      const result=await timeout(Promise.resolve().then(()=>h.hook(api(h.mod),Object.freeze({...e}),next)));
      if(once)await once;
      if(result===undefined)throw new Error('Mod hook returned no result');
      return result;
    } catch(error) {
      if(!h.catch)throw new Error(`${h.mod.name}: ${error.message}`);
      next.error={kind:'throw',message:String(error)};next.called=called;
      const result=await timeout(Promise.resolve(h.catch(api(h.mod),e,next)),1000);if(once)await once;return result;
    }
  };
  return run(0,input);
}

async function initialize(input) {
  session=input.session;home=input.home;
  for(const metadata of input.mods){
    const module=await import(pathToFileURL(metadata.entry).href);
    const definition=module.default?.definition??module.default??module;
    const registerHooks=definition.register??module.register;
    if(typeof registerHooks!=='function')throw new Error(metadata.name+': Mod must export register(on, options) or defineMod');
    const mod={...metadata,root:metadata.root??path.dirname(metadata.entry)};
    if(definition.name&&definition.name!==mod.name)throw new Error('Mod definition name does not match manifest');
    const on=(event,matcher,hook)=>{if(typeof matcher==='function'){hook=matcher;matcher={}}if(!supported.has(event))throw new Error('AX does not raise Mod event '+event);if(typeof hook!=='function')throw new Error('Invalid Mod hook');const registration={event,matcher,hook,mod};hooks.push(registration);return {catch:handler=>{registration.catch=handler}}};
    await registerHooks(on,{...(definition.options??definition.userConfig??{}),...metadata.userConfig});
  }
  await dispatch('session.start',{cwd:session.cwd,surface:null,isInteractive:false},async e=>({cwd:e.cwd}));
  return {tools:[...tools.values()],commands:[...commands.values()]};
}

const lines=createInterface({input:process.stdin,crlfDelay:Infinity});
lines.on('line',line=>{
  let request;try{request=JSON.parse(line)}catch(error){console.error(error);return}
  if(request.reply){const waiter=pending.get(request.reply);if(waiter){pending.delete(request.reply);request.error?waiter.reject(new Error(request.error)):waiter.resolve(request.result)}return}
  if(active){send({error:'Concurrent Mod bridge request'});return}
  active=true;
  (async()=>{
    if(request.event==='initialize')return initialize(request.input);
    if(request.event==='context.update'){messages=request.input.messages;session.window=request.input.window;return {}}
    if(request.event==='prompt.submit'){
      Object.assign(session,request.input.session??{});messages=request.input.messages??messages;started=Date.now();turns++;
      await dispatch('turn.start',{turn:turns,agentId:null},async()=>({}));
      const command=request.input.text.match(/^\/mod:([\w.-]+)(?:\s+([\s\S]*))?$/);
      if(command){if(!commands.has(command[1]))throw new Error('Unknown Mod command: '+command[1]);const response=await dispatch('command.run',{command:command[1],args:command[2]??'',origin:{kind:'user'}},()=>{throw new Error('No Mod answered command '+command[1])});return {text:request.input.text,context:[],response:response.text??''}}
      return dispatch('prompt.submit',{text:request.input.text,context:[],origin:{kind:'user'}},async e=>e);
    }
    if(request.event==='tool.call')return dispatch('tool.call',request.input,async e=>host('next',e));
    if(request.event==='turn.complete'){await dispatch('turn.complete',{...request.input,durationMs:Date.now()-started,turn:turns,agentId:null},async()=>({}));return {}}
    if(request.event==='session.end'){for(const timer of timers){clearTimeout(timer);clearInterval(timer)}timers.clear();await dispatch('session.end',{reason:'disposed'},async()=>({}));return {}}
    throw new Error('Unknown bridge event '+request.event);
  })().then(result=>send({result}),error=>send({error:String(error.stack??error)})).finally(()=>{active=false});
});
lines.on('close',()=>{for(const timer of timers){clearTimeout(timer);clearInterval(timer)}process.exit(0)});
