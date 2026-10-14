"""macOS AX accessibility and target-window-only Quartz/ScreenCaptureKit capture."""
import os
import tempfile
import time
from worker import LIMIT, DEPTH, short

class Backend:
    def __init__(self):
        try:
            import ApplicationServices as AX
            import Quartz as Q
            import AppKit as K
            import Foundation as F
            from CoreFoundation import CFEqual
        except ImportError as error:
            raise RuntimeError('Install PyObjC ApplicationServices, Quartz, Cocoa and ScreenCaptureKit for macOS Computer Use') from error
        self.ax,self.q,self.k,self.f,self.equal = AX,Q,K,F,CFEqual
        if not AX.AXIsProcessTrusted():
            raise PermissionError('macOS Accessibility permission is required for this Python/AX launch context; grant it in System Settings > Privacy & Security > Accessibility')

    def attr(self, element, name, optional=False):
        error,value = self.ax.AXUIElementCopyAttributeValue(element,name,None)
        if error == 0: return value
        if optional and error in (-25205,-25212): return None
        raise RuntimeError(f'Accessibility attribute {name} unavailable ({error})')

    def children(self, element):
        error,count = self.ax.AXUIElementGetAttributeValueCount(element,'AXChildren',None)
        if error in (-25205,-25212): return []
        if error != 0 or count > LIMIT: raise RuntimeError('Incomplete accessibility children scan')
        if count == 0: return []
        error,children = self.ax.AXUIElementCopyAttributeValues(element,'AXChildren',0,count,None)
        if error != 0: raise RuntimeError('Cannot read accessibility children')
        return list(children or [])

    def bounds(self, element):
        position,size = self.attr(element,'AXPosition'),self.attr(element,'AXSize')
        ok,point = self.ax.AXValueGetValue(position,self.ax.kAXValueCGPointType,None)
        valid,dimensions = self.ax.AXValueGetValue(size,self.ax.kAXValueCGSizeType,None)
        if not ok or not valid: raise RuntimeError('Accessibility bounds unavailable')
        return dict(x=float(point[0]),y=float(point[1]),width=float(dimensions[0]),height=float(dimensions[1]))

    def windows(self, hidden):
        options = self.q.kCGWindowListExcludeDesktopElements
        if not hidden: options |= self.q.kCGWindowListOptionOnScreenOnly
        records = self.q.CGWindowListCopyWindowInfo(options,self.q.kCGNullWindowID) or []
        applications = {}; entries = []
        for record in records:
            if record.get(self.q.kCGWindowLayer) != 0: continue
            pid = int(record[self.q.kCGWindowOwnerPID])
            try:
                application = self.k.NSRunningApplication.runningApplicationWithProcessIdentifier_(pid)
                if application is None or application.executableURL() is None: continue
                path = os.path.realpath(str(application.executableURL().path()))
                if not os.path.isfile(path): continue
                if pid not in applications:
                    app = self.ax.AXUIElementCreateApplication(pid)
                    self.ax.AXUIElementSetMessagingTimeout(app,1.0)
                    applications[pid] = list(self.attr(app,'AXWindows',True) or [])
                rect = record[self.q.kCGWindowBounds]
                matches = []
                for window in applications[pid]:
                    bounds = self.bounds(window)
                    if all(abs(bounds[key]-float(rect[name]))<=2 for key,name in [('x','X'),('y','Y'),('width','Width'),('height','Height')]): matches.append(window)
                if len(matches)!=1: continue
                identifier = f'cg:{pid}:{int(record[self.q.kCGWindowNumber])}'
                entries.append((matches[0],dict(id=identifier, title=short(record.get(self.q.kCGWindowName,'')),
                    visible=bool(record.get(self.q.kCGWindowIsOnscreen)), process_id=pid,app_id=path,app_name=os.path.basename(path))))
            except Exception: continue  # Transient/non-accessible windows are not actionable.
            if len(entries)>=LIMIT: raise RuntimeError('Desktop window limit exceeded')
        return entries

    def describe(self, element):
        try:
            role = str(self.attr(element,'AXRole'))
            subrole = self.attr(element,'AXSubrole',True)
            password = subrole == 'AXSecureTextField' or role == 'AXSecureTextField' or self.attr(element,'AXProtectedContent',True) is True
            result = dict(role=role,name='[password]' if password else short(self.attr(element,'AXTitle',True) or self.attr(element,'AXDescription',True)),
                password=password,enabled=bool(self.attr(element,'AXEnabled',True)),focused=bool(self.attr(element,'AXFocused',True)),
                native_id=short(self.attr(element,'AXIdentifier',True)))
            try: result['bounds']=self.bounds(element)
            except Exception: pass
            if not password:
                value = self.attr(element,'AXValue',True)
                if value is not None: result['value']=short(value)
            return result
        except Exception: return dict(role='unavailable',name='[unreadable]',password=True,unreadable=True)

    def pid(self, element):
        error,pid = self.ax.AXUIElementGetPid(element,None)
        if error != 0: raise RuntimeError('Cannot verify accessibility process')
        return int(pid)

    def belongs(self, element, window):
        for _ in range(DEPTH):
            if self.equal(element,window): return True
            element = self.attr(element,'AXParent',True)
            if element is None: return False
        return False

    def focused(self, window):
        app = self.ax.AXUIElementCreateApplication(self.pid(window))
        focused = self.attr(app,'AXFocusedUIElement')
        if not self.belongs(focused,window): raise RuntimeError('Focused control is outside target window')
        return focused

    def active(self, window):
        pid = self.pid(window)
        app = self.k.NSWorkspace.sharedWorkspace().frontmostApplication()
        if app is None or int(app.processIdentifier())!=pid: raise RuntimeError('Target application is not foreground; use focus_window')
        focused = self.attr(self.ax.AXUIElementCreateApplication(pid),'AXFocusedWindow')
        if not self.equal(focused,window): raise RuntimeError('A different window is focused')

    def focus(self, window):
        application = self.k.NSRunningApplication.runningApplicationWithProcessIdentifier_(self.pid(window))
        if not application.activateWithOptions_(self.k.NSApplicationActivateIgnoringOtherApps): raise RuntimeError('macOS refused application focus')
        error = self.ax.AXUIElementPerformAction(window,'AXRaise')
        if error != 0: raise RuntimeError(f'Window refused raise ({error})')
        for _ in range(20):
            try: self.active(window); return
            except RuntimeError: time.sleep(.025)
        raise RuntimeError('Window did not become foreground')

    def invoke(self, element):
        error,actions = self.ax.AXUIElementCopyActionNames(element,None)
        if error != 0: raise RuntimeError('Element actions are unavailable')
        for action in ['AXPress','AXConfirm','AXPick','AXShowMenu','AXIncrement']:
            if action in actions:
                error = self.ax.AXUIElementPerformAction(element,action)
                if error != 0: raise RuntimeError(f'Application refused action ({error})')
                return
        raise RuntimeError('Element has no supported accessibility action')

    def type_text(self, window, element, text):
        error,settable = self.ax.AXUIElementIsAttributeSettable(element,'AXValue',None)
        if error == 0 and settable:
            error = self.ax.AXUIElementSetAttributeValue(element,'AXValue',text)
            if error != 0: raise RuntimeError(f'Application refused text ({error})')
            return
        self.active(window)
        error = self.ax.AXUIElementSetAttributeValue(element,'AXFocused',True)
        if error != 0: raise RuntimeError('Cannot focus target control for text input')
        self.active(window)
        if not self.equal(self.focused(window),element): raise RuntimeError('Target control did not receive focus')
        # Unicode events target only the verified foreground application.
        down = self.q.CGEventCreateKeyboardEvent(None,0,True)
        up = self.q.CGEventCreateKeyboardEvent(None,0,False)
        self.q.CGEventKeyboardSetUnicodeString(down,len(text.encode('utf-16-le'))//2,text)
        self.q.CGEventPostToPid(self.pid(window),down)
        self.q.CGEventPostToPid(self.pid(window),up)

    def press_key(self, window, data):
        self.active(window)
        letters = {'A':0,'S':1,'D':2,'F':3,'H':4,'G':5,'Z':6,'X':7,'C':8,'V':9,'B':11,
            'Q':12,'W':13,'E':14,'R':15,'Y':16,'T':17,'1':18,'2':19,'3':20,'4':21,'6':22,'5':23,
            '9':25,'7':26,'8':28,'0':29,'O':31,'U':32,'I':34,'P':35,'L':37,'J':38,'K':40,'N':45,'M':46}
        keys = dict(Return=36,Enter=36,Escape=53,Esc=53,Tab=48,Backspace=51,Delete=117,Left=123,Right=124,Down=125,Up=126,Home=115,End=119,PageUp=116,PageDown=121,Space=49)
        keys.update({f'F{i}':code for i,code in enumerate([122,120,99,118,96,97,98,100,101,109,103,111],1)})
        key = data.get('key')
        code = letters.get(key.upper()) if isinstance(key,str) and len(key)==1 else {name.upper():value for name,value in keys.items()}.get(key.upper() if isinstance(key,str) else None)
        if code is None: raise ValueError('Unsupported key')
        modifiers = data.get('modifiers',[])
        flags = dict(ctrl=self.q.kCGEventFlagMaskControl,shift=self.q.kCGEventFlagMaskShift,alt=self.q.kCGEventFlagMaskAlternate,cmd=self.q.kCGEventFlagMaskCommand,meta=self.q.kCGEventFlagMaskCommand)
        if not isinstance(modifiers,list) or any(value not in flags for value in modifiers): raise ValueError('Unsupported modifier')
        mask = 0
        for value in set(modifiers): mask |= flags[value]
        for down in [True,False]:
            event = self.q.CGEventCreateKeyboardEvent(None,code,down)
            self.q.CGEventSetFlags(event,mask)
            self.q.CGEventPostToPid(self.pid(window),event)

    def mouse(self, window, data):
        self.active(window)
        x,y = data.get('x'),data.get('y'); rect=self.bounds(window)
        if type(x) is not int or type(y) is not int: raise ValueError('Coordinates must be integers')
        if not (rect['x']<=x<rect['x']+rect['width'] and rect['y']<=y<rect['y']+rect['height']): raise ValueError('Coordinates outside target window')
        error,hit = self.ax.AXUIElementCopyElementAtPosition(self.ax.AXUIElementCreateSystemWide(),x,y,None)
        if error!=0 or not self.belongs(hit,window): raise RuntimeError('Coordinates covered by another window')
        point=(x,y); self.q.CGEventPost(self.q.kCGHIDEventTap,self.q.CGEventCreateMouseEvent(None,self.q.kCGEventMouseMoved,point,self.q.kCGMouseButtonLeft))
        if data['action']=='mouse_click':
            buttons=dict(left=(self.q.kCGMouseButtonLeft,self.q.kCGEventLeftMouseDown,self.q.kCGEventLeftMouseUp),right=(self.q.kCGMouseButtonRight,self.q.kCGEventRightMouseDown,self.q.kCGEventRightMouseUp),middle=(self.q.kCGMouseButtonCenter,self.q.kCGEventOtherMouseDown,self.q.kCGEventOtherMouseUp))
            if data.get('button') not in buttons: raise ValueError('Unsupported button')
            button,down,up=buttons[data['button']]; self.active(window)
            error,hit=self.ax.AXUIElementCopyElementAtPosition(self.ax.AXUIElementCreateSystemWide(),x,y,None)
            if error!=0 or not self.belongs(hit,window): raise RuntimeError('Coordinates became covered')
            self.q.CGEventPost(self.q.kCGHIDEventTap,self.q.CGEventCreateMouseEvent(None,down,point,button))
            self.q.CGEventPost(self.q.kCGHIDEventTap,self.q.CGEventCreateMouseEvent(None,up,point,button))

    def await_callback(self, starter):
        values=[]
        starter(lambda value,error: values.append((value,error)))
        deadline=time.monotonic()+6
        while not values and time.monotonic()<deadline:
            self.f.NSRunLoop.currentRunLoop().runUntilDate_(self.f.NSDate.dateWithTimeIntervalSinceNow_(.02))
        if not values: raise RuntimeError('macOS capture timed out')
        value,error=values[0]
        if error is not None: raise RuntimeError(short(error))
        return value

    def capture(self, window, entry, limit):
        if not self.q.CGPreflightScreenCaptureAccess(): raise PermissionError('Grant macOS Screen Recording permission before capturing windows')
        identifier=int(entry['id'].rsplit(':',1)[1]); rect=self.bounds(window)
        w,h=rect['width'],rect['height']
        if w<=0 or h<=0 or w>8192 or h>8192 or w*h>32000000: raise RuntimeError('Window exceeds capture bounds')
        try: import ScreenCaptureKit as S
        except ImportError: S=None
        if S is not None and hasattr(S,'SCScreenshotManager'):
            content=self.await_callback(S.SCShareableContent.getShareableContentWithCompletionHandler_)
            candidates=[item for item in content.windows() if int(item.windowID())==identifier and int(item.owningApplication().processID())==entry['process_id']]
            if len(candidates)!=1: raise RuntimeError('Capture window no longer exists')
            filter=S.SCContentFilter.alloc().initWithDesktopIndependentWindow_(candidates[0])
            config=S.SCStreamConfiguration.alloc().init()
            scale=min(1.0,limit/max(w,h)); config.setWidth_(max(1,int(w*scale))); config.setHeight_(max(1,int(h*scale)))
            config.setShowsCursor_(False)
            image=self.await_callback(lambda callback:S.SCScreenshotManager.captureImageWithFilter_configuration_completionHandler_(filter,config,callback))
        else:
            # Older macOS: still only the explicit target window, never a screen crop.
            image=self.q.CGWindowListCreateImage(self.q.CGRectNull,self.q.kCGWindowListOptionIncludingWindow,identifier,self.q.kCGWindowImageBoundsIgnoreFraming | self.q.kCGWindowImageNominalResolution)
        if image is None: raise RuntimeError('macOS refused target-window capture')
        width,height=self.q.CGImageGetWidth(image),self.q.CGImageGetHeight(image)
        if max(width,height)>limit:
            ratio=limit/max(width,height); width,height=max(1,int(width*ratio)),max(1,int(height*ratio))
            context=self.q.CGBitmapContextCreate(None,width,height,8,width*4,self.q.CGColorSpaceCreateDeviceRGB(),self.q.kCGImageAlphaPremultipliedLast)
            if context is None: raise RuntimeError('Cannot resize window image')
            self.q.CGContextDrawImage(context,((0,0),(width,height)),image); image=self.q.CGBitmapContextCreateImage(context)
        fd,path=tempfile.mkstemp(prefix='ax-desktop-',suffix='.png'); os.close(fd)
        destination=self.q.CGImageDestinationCreateWithURL(self.f.NSURL.fileURLWithPath_(path),'public.png',1,None)
        if destination is None: raise RuntimeError('Cannot encode window PNG')
        self.q.CGImageDestinationAddImage(destination,image,None)
        if not self.q.CGImageDestinationFinalize(destination): raise RuntimeError('Cannot save window PNG')
        return path
