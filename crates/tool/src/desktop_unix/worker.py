"""AX's disposable desktop adapter. No model-supplied code or shell is executed."""
import collections
import hashlib
import json
import os
import sys

LIMIT = 20000
DEPTH = 128

def short(value):
    return str(value or '')[:2048]

def unique_object(pairs):
    value={}
    for key,item in pairs:
        if key in value: raise PermissionError('Duplicate application permission field')
        value[key]=item
    return value

def fingerprint(node):
    # Accessibility paths plus current role/name/bounds reject changed targets.
    # Password values are never inspected, even for an identity fingerprint.
    value = {key: node.get(key) for key in ('role', 'name', 'bounds', 'native_id')}
    return hashlib.sha256(json.dumps(value, sort_keys=True).encode()).hexdigest()[:24]

def inspect(backend, window, limit):
    queue = collections.deque([(window, [], None, 0)])
    nodes = []
    objects = {}
    truncated = False
    privacy = True
    while queue:
        element, path, parent, depth = queue.popleft()
        node = backend.describe(element)
        identifier = '.'.join(map(str, path)) + ':' + fingerprint(node)
        node.pop('native_id', None)
        node.update(id=identifier, parent_id=parent)
        nodes.append(node)
        objects[identifier] = element
        privacy &= not node.get('password', True) and not node.get('unreadable', False)
        children = backend.children(element)
        for index, child in enumerate(children):
            if depth >= DEPTH or len(nodes) + len(queue) >= limit:
                truncated = True
                break
            queue.append((child, path + [index], identifier, depth + 1))
    return {'nodes': nodes, 'node_limit': limit, 'truncated': truncated}, objects, privacy and not truncated

def locate(backend, window, identifier):
    if not isinstance(identifier, str) or len(identifier) > 2048:
        raise ValueError('Missing or invalid element_id')
    path, expected = identifier.rsplit(':', 1)
    element = window
    indices = [] if not path else [int(value) for value in path.split('.')]
    if len(indices) > DEPTH: raise ValueError('Element path exceeds depth limit')
    for index in indices:
        children = backend.children(element)
        if index < 0 or index >= len(children): raise ValueError('Element no longer exists; read the window again')
        element = children[index]
    if fingerprint(backend.describe(element)) != expected:
        raise ValueError('Element changed; read the window again')
    return element

def run(request, backend):
    data = request['input']
    action = data['action']
    allowed = {'list_windows', 'read_window', 'screenshot_window', 'focus_window', 'click_element', 'type_text', 'press_key', 'move_mouse', 'mouse_click'}
    if action not in allowed: raise ValueError('Unsupported desktop action')
    windows = backend.windows(bool(data.get('include_hidden', False)))
    if action == 'list_windows':
        return {'status': 'windows_listed', 'windows': [entry for _, entry in windows]}
    selected = [(window, entry) for window, entry in windows if entry['id'] == data.get('window_id')]
    if len(selected) != 1: raise ValueError('Window no longer exists; list windows again')
    window, entry = selected[0]
    if request.get('resolve_only'):
        return {key: entry[key] for key in ('app_id', 'app_name')}
    if os.path.realpath(entry['app_id']) != request.get('authorized_app'):
        raise PermissionError('Window application changed; request app access again')
    # Rust owns the grant. Recheck only persistent denial immediately before
    # effects, after native discovery; no Python rule can create a grant.
    if request.get('policy_path'):
        try:
            with open(request['policy_path'],encoding='utf-8') as file: policy=json.load(file,object_pairs_hook=unique_object)
        except FileNotFoundError: policy={}
        if not isinstance(policy,dict):
            raise PermissionError('Invalid application permission policy')
        valid = set(policy)<= {'browser_enabled','apps','sites','versions'}
        valid &= type(policy.get('browser_enabled',True)) is bool
        for field in ('apps','sites'):
            rules=policy.get(field,{})
            valid &= isinstance(rules,dict) and all(isinstance(key,str) and decision in ('allow','ask','deny') for key,decision in rules.items())
        versions=policy.get('versions',{})
        valid &= isinstance(versions,dict) and all(isinstance(key,str) and isinstance(value,str) for key,value in versions.items())
        if not valid:
            raise PermissionError('Invalid application permission policy')
        if policy.get('apps',{}).get(os.path.realpath(entry['app_id'])) == 'deny':
            raise PermissionError('Application access was revoked')
    result = {'status': 'completed', 'window_id': entry['id']}
    if action == 'read_window':
        result['status'] = 'window_read'
        result['ui_tree'], _, _ = inspect(backend, window, min(int(request['max_nodes']), 10000))
    elif action == 'focus_window': backend.focus(window)
    elif action == 'click_element': backend.invoke(locate(backend, window, data.get('element_id')))
    elif action == 'type_text':
        text = data.get('text')
        if not isinstance(text, str): raise ValueError('text must be a string')
        element = locate(backend, window, data['element_id']) if 'element_id' in data else backend.focused(window)
        backend.type_text(window, element, text)
    elif action == 'press_key': backend.press_key(window, data)
    elif action in ('move_mouse', 'mouse_click'): backend.mouse(window, data)
    if action in ('read_window', 'screenshot_window'):
        if not request.get('include_screenshot'):
            result['screenshot_skipped'] = 'disabled'
        else:
            try:
                _, _, safe = inspect(backend, window, LIMIT)
                if not safe: result['screenshot_skipped'] = 'password_or_incomplete_privacy_scan'
                else: result['screenshot_path'] = backend.capture(window, entry, int(request['screenshot_width']))
            except Exception as error:
                result.update(screenshot_skipped='privacy_scan_or_capture_failed', screenshot_message=short(error))
    return result

def main():
    sys.path.insert(0, os.path.dirname(__file__))
    try:
        request = json.load(sys.stdin)
        if sys.platform == 'linux':
            from linux import Backend
        elif sys.platform == 'darwin':
            from macos import Backend
        else: raise ValueError('No Unix desktop adapter for this platform')
        result = run(request, Backend())
    except Exception as error:
        result = {'error': short(error)}
    print(json.dumps(result, ensure_ascii=False))

if __name__ == '__main__': main()
