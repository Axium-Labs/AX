"""Native AT-SPI actions; X11 input/capture require a verified X server PID."""
import hashlib
import json
import os
import tempfile
import time
from worker import LIMIT, DEPTH, short, fingerprint

class Backend:
    def __init__(self):
        try:
            import gi
            gi.require_version('Atspi', '2.0')
            from gi.repository import Atspi
        except ImportError as error:
            raise RuntimeError('Install Python 3 PyGObject and gir1.2-atspi-2.0 for Linux Computer Use') from error
        if not (os.environ.get('DISPLAY') or os.environ.get('WAYLAND_DISPLAY')):
            raise RuntimeError('Linux Computer Use requires a graphical desktop session')
        self.a = Atspi
        Atspi.init()
        Atspi.set_timeout(1000, 1000)
        self.desktop = Atspi.get_desktop(0)
        if self.desktop is None: raise RuntimeError('AT-SPI accessibility bus is unavailable')

    def children(self, node):
        count = node.get_child_count()
        if count < 0 or count > LIMIT: raise RuntimeError('Accessibility children exceed scan limit')
        return [node.get_child_at_index(index) for index in range(count)]

    def native_id(self, node):
        return str(node.app.bus_name) + '|' + str(node.path)

    def bounds(self, node):
        rect = node.get_component_iface().get_extents(self.a.CoordType.SCREEN)
        return dict(x=rect.x, y=rect.y, width=rect.width, height=rect.height)

    def windows(self, hidden):
        entries = []
        for app in self.children(self.desktop):
            for window in self.children(app):
                try:
                    if window.get_role() not in (self.a.Role.FRAME, self.a.Role.DIALOG, self.a.Role.WINDOW): continue
                    visible = window.get_state_set().contains(self.a.StateType.SHOWING)
                    if not visible and not hidden: continue
                    # get_process_id queries GetConnectionUnixProcessID on the
                    # accessibility connection; never trust an app-provided PID.
                    pid = int(window.get_process_id())
                    path = os.path.realpath(f'/proc/{pid}/exe')
                    if pid <= 0 or not os.path.isfile(path): continue
                    entries.append((window, dict(id=self.native_id(window), title=short(window.get_name()), visible=visible,
                        process_id=pid, app_id=path, app_name=os.path.basename(path))))
                except Exception: continue  # A window may disappear during enumeration.
                if len(entries) >= LIMIT: raise RuntimeError('Desktop window limit exceeded')
        return entries

    def describe(self, node):
        try:
            role = node.get_role()
            attrs = node.get_attributes() or {}
            password = role == self.a.Role.PASSWORD_TEXT or attrs.get('text-input-type') == 'password'
            states = node.get_state_set()
            value = dict(role=short(node.get_role_name()), name='[password]' if password else short(node.get_name()),
                password=password, enabled=states.contains(self.a.StateType.ENABLED),
                focused=states.contains(self.a.StateType.FOCUSED), native_id=self.native_id(node))
            try: value['bounds'] = self.bounds(node)
            except Exception: pass
            if not password and 'Text' in node.get_interfaces():
                text = node.get_text_iface()
                value['value'] = short(self.a.Text.get_text(text, 0, min(self.a.Text.get_character_count(text), 2048)))
            return value
        except Exception:
            return dict(role='unavailable', name='[unreadable]', password=True, unreadable=True)

    def focused(self, window):
        queue = [(window, 0)]
        seen = 0
        while queue:
            node, depth = queue.pop()
            seen += 1
            if seen > LIMIT or depth > DEPTH: raise RuntimeError('Focus scan exceeds limit')
            if node.get_state_set().contains(self.a.StateType.FOCUSED): return node
            queue.extend((child, depth + 1) for child in self.children(node))
        raise RuntimeError('No focused control inside the target window')

    def invoke(self, node):
        if 'Action' not in node.get_interfaces(): raise RuntimeError('Element has no accessibility action')
        action = node.get_action_iface()
        for index in range(min(action.get_n_actions(), 64)):
            if action.get_action_name(index).lower() in ('click', 'press', 'activate', 'toggle', 'select', 'expand', 'open'):
                if not action.do_action(index): raise RuntimeError('Application refused the accessibility action')
                return
        raise RuntimeError('Element has no supported accessibility action')

    def type_text(self, window, node, text):
        if 'EditableText' not in node.get_interfaces(): raise RuntimeError('Target has no EditableText interface; no unscoped keyboard fallback is used')
        if not node.get_editable_text_iface().set_text_contents(text): raise RuntimeError('Application refused text input')

    def x11(self):
        if os.environ.get('WAYLAND_DISPLAY'):
            raise RuntimeError('Wayland coordinate input/window capture is unavailable; use accessibility actions')
        try:
            from Xlib import display
            return display.Display()
        except ImportError as error: raise RuntimeError('Install python-xlib for X11 input and capture') from error

    def xwindow(self, connection, window):
        from Xlib import X
        from Xlib.ext import res
        if not connection.has_extension('X-Resource'): raise RuntimeError('XRes is required to verify target X11 ownership')
        root = connection.screen().root
        bounds = self.bounds(window)
        property = root.get_full_property(connection.intern_atom('_NET_CLIENT_LIST'), X.AnyPropertyType)
        ids = property.value if property is not None else [child.id for child in root.query_tree().children]
        matches = []
        for identifier in ids:
            try:
                target = connection.create_resource_object('window', int(identifier))
                owners = connection.res_query_client_ids([{'client': int(identifier), 'mask': res.LocalClientPIDMask}]).ids
                if len(owners) != 1 or not owners[0].value or int(owners[0].value[0]) != int(window.get_process_id()): continue
                size = target.get_geometry(); origin = root.translate_coords(target, 0, 0)
                rectangles = [(origin.x,origin.y,size.width,size.height)]
                frame = target
                while frame.query_tree().parent.id != root.id: frame = frame.query_tree().parent
                frame_size = frame.get_geometry(); frame_origin = root.translate_coords(frame,0,0)
                rectangles.append((frame_origin.x,frame_origin.y,frame_size.width,frame_size.height))
                if any(all(abs(actual-bounds[key])<=4 for actual,key in zip(rectangle,['x','y','width','height'])) for rectangle in rectangles):
                    matches.append(target)
            except Exception: continue
        if len(matches) != 1: raise RuntimeError('Cannot uniquely verify the target X11 window and process')
        return matches[0]

    def active(self, connection, target):
        from Xlib import X
        root = connection.screen().root
        active = root.get_full_property(connection.intern_atom('_NET_ACTIVE_WINDOW'), X.AnyPropertyType)
        if active is None or not active.value or int(active.value[0]) != target.id:
            raise RuntimeError('Target window is not foreground; call focus_window first')

    def focus(self, window):
        if os.environ.get('WAYLAND_DISPLAY'):
            if not window.get_component_iface().grab_focus(): raise RuntimeError('Compositor/application refused focus')
            if not window.get_state_set().contains(self.a.StateType.ACTIVE): raise RuntimeError('Wayland target did not become active')
            return
        from Xlib import X, protocol
        connection = self.x11()
        try:
            target = self.xwindow(connection, window)
            event = protocol.event.ClientMessage(window=target, client_type=connection.intern_atom('_NET_ACTIVE_WINDOW'), data=(32, [2, X.CurrentTime, 0, 0, 0]))
            connection.screen().root.send_event(event, event_mask=X.SubstructureRedirectMask | X.SubstructureNotifyMask)
            connection.flush()
            for _ in range(20):
                try: self.active(connection, target); return
                except RuntimeError: time.sleep(.025)
            raise RuntimeError('Window manager refused focus')
        finally: connection.close()

    def press_key(self, window, data):
        from Xlib import X, XK
        from Xlib.ext import xtest
        aliases = {'enter':'Return','return':'Return','esc':'Escape','escape':'Escape','tab':'Tab','delete':'Delete','left':'Left','right':'Right','up':'Up','down':'Down','home':'Home','end':'End','pageup':'Prior','pagedown':'Next','backspace':'BackSpace','space':'space'}
        key = data.get('key')
        if isinstance(key,str) and len(key)>1: key = aliases.get(key.lower(),key.upper())
        valid = {'Return','Escape','Tab','BackSpace','Delete','Left','Right','Up','Down','Home','End','Prior','Next','space'} | {f'F{i}' for i in range(1,13)}
        if not isinstance(key, str) or not (key in valid or len(key) == 1 and key.isascii() and key.isalnum()): raise ValueError('Unsupported key')
        modifiers = data.get('modifiers', [])
        if not isinstance(modifiers, list) or any(value not in ('ctrl','shift','alt','meta') for value in modifiers): raise ValueError('Unsupported modifier; cmd is macOS-only')
        connection = self.x11(); pressed = []
        try:
            target = self.xwindow(connection, window); self.active(connection, target)
            names = dict(ctrl='Control_L',shift='Shift_L',alt='Alt_L',meta='Super_L')
            codes = [connection.keysym_to_keycode(XK.string_to_keysym(names[value])) for value in dict.fromkeys(modifiers)]
            code = connection.keysym_to_keycode(XK.string_to_keysym(key.lower() if len(key)==1 else key))
            if not code or any(not value for value in codes): raise RuntimeError('Key is not mapped in this keyboard layout')
            for value in codes: xtest.fake_input(connection, X.KeyPress, value); pressed.append(value)
            self.active(connection, target)
            xtest.fake_input(connection, X.KeyPress, code); pressed.append(code)
            connection.sync()
        finally:
            for value in reversed(pressed): xtest.fake_input(connection, X.KeyRelease, value)
            connection.sync(); connection.close()

    def unobscured(self, connection, target):
        root = connection.screen().root
        frame = target
        while frame.query_tree().parent.id != root.id: frame = frame.query_tree().parent
        siblings = root.query_tree().children
        index = next(i for i, sibling in enumerate(siblings) if sibling.id == frame.id)
        size = target.get_geometry(); origin = root.translate_coords(target, 0, 0)
        for other in siblings[index+1:]:
            if other.get_attributes().map_state != 2: continue
            rect = other.get_geometry(); point = root.translate_coords(other, 0, 0)
            if point.x < origin.x + size.width and point.x + rect.width > origin.x and point.y < origin.y + size.height and point.y + rect.height > origin.y:
                raise RuntimeError('Target is covered by another window')

    def mouse(self, window, data):
        from Xlib import X
        from Xlib.ext import xtest
        x,y = data.get('x'),data.get('y')
        if type(x) is not int or type(y) is not int: raise ValueError('Coordinates must be integers')
        rect = self.bounds(window)
        if not (rect['x'] <= x < rect['x']+rect['width'] and rect['y'] <= y < rect['y']+rect['height']): raise ValueError('Coordinates are outside target window')
        connection = self.x11()
        try:
            target = self.xwindow(connection, window); self.active(connection,target); self.unobscured(connection,target)
            xtest.fake_input(connection,X.MotionNotify,x=x,y=y); connection.sync()
            if data['action'] == 'mouse_click':
                button = {'left':1,'middle':2,'right':3}.get(data.get('button'))
                if button is None: raise ValueError('Unsupported mouse button')
                self.active(connection,target); self.unobscured(connection,target)
                xtest.fake_input(connection,X.ButtonPress,button)
                try: connection.sync()
                finally: xtest.fake_input(connection,X.ButtonRelease,button); connection.sync()
        finally: connection.close()

    def capture(self, window, entry, limit):
        from Xlib import X
        from PIL import Image
        connection = self.x11()
        try:
            target = self.xwindow(connection,window); self.unobscured(connection,target)
            rect = target.get_geometry(); w,h = rect.width,rect.height
            if w<=0 or h<=0 or w>8192 or h>8192 or w*h>32000000: raise RuntimeError('Window exceeds capture bounds')
            pixels = target.get_image(0,0,w,h,X.ZPixmap,0xffffffff)
            visual_id = target.get_attributes().visual
            visuals = [visual for depth in connection.screen().allowed_depths for visual in depth.visuals if visual.visual_id==visual_id]
            if len(visuals)!=1 or (visuals[0].red_mask,visuals[0].green_mask,visuals[0].blue_mask)!=(0xff0000,0xff00,0xff): raise RuntimeError('Unsupported X11 TrueColor masks')
            # TrueColor depth 24/32 is the supported native X11 capture format.
            formats = [item for item in connection.display.info.pixmap_formats if item.depth == rect.depth]
            if len(formats)!=1 or formats[0].bits_per_pixel!=32: raise RuntimeError('Unsupported X11 pixel format')
            image = Image.frombytes('RGB',(w,h),pixels.data,'raw','BGRX' if connection.display.info.image_byte_order==0 else 'XRGB')
            image.thumbnail((limit,limit),Image.Resampling.LANCZOS)
            fd,path = tempfile.mkstemp(prefix='ax-desktop-',suffix='.png'); os.close(fd)
            image.save(path)
            return path
        finally: connection.close()
