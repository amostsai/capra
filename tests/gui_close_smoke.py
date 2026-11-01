#!/usr/bin/env python3
"""Real X11 GUI test on an isolated Xvfb display. Requires Xvfb, libXtst, ffprobe.
Run after cargo build: python3 tests/gui_close_smoke.py target/debug/capra
Does not capture the user's screen, change desktop settings, or use their config.
"""
import ctypes as C
import ctypes.util
import json
import os
from pathlib import Path
import socket
import subprocess as S
import sys
import tempfile
import time

binary = str(Path(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/capra').resolve())
probe = socket.socket(socket.AF_UNIX)
try:
    probe.connect('/tmp/capra_amos.sock')
except OSError:
    pass
else:
    raise SystemExit('A recorder is already running; smoke test will not interfere with it.')
finally:
    probe.close()
x = C.CDLL(ctypes.util.find_library('X11'))
xt = C.CDLL(ctypes.util.find_library('Xtst'))
x.XOpenDisplay.argtypes = [C.c_char_p]
x.XOpenDisplay.restype = C.c_void_p
x.XDefaultRootWindow.argtypes = [C.c_void_p]
x.XDefaultRootWindow.restype = C.c_ulong
x.XQueryTree.argtypes = [C.c_void_p,C.c_ulong,C.POINTER(C.c_ulong),C.POINTER(C.c_ulong),C.POINTER(C.POINTER(C.c_ulong)),C.POINTER(C.c_uint)]
x.XFetchName.argtypes = [C.c_void_p,C.c_ulong,C.POINTER(C.c_char_p)]
x.XFree.argtypes = [C.c_void_p]
x.XSetInputFocus.argtypes = [C.c_void_p,C.c_ulong,C.c_int,C.c_ulong]
x.XStringToKeysym.argtypes = [C.c_char_p]
x.XStringToKeysym.restype = C.c_ulong
x.XKeysymToKeycode.argtypes = [C.c_void_p,C.c_ulong]
x.XKeysymToKeycode.restype = C.c_uint
x.XkbKeycodeToKeysym.argtypes = [C.c_void_p,C.c_uint,C.c_uint,C.c_uint]
x.XkbKeycodeToKeysym.restype = C.c_ulong
x.XInternAtom.argtypes = [C.c_void_p,C.c_char_p,C.c_int]
x.XInternAtom.restype = C.c_ulong
x.XFlush.argtypes = [C.c_void_p]
x.XChangeProperty.argtypes = [C.c_void_p,C.c_ulong,C.c_ulong,C.c_ulong,C.c_int,C.c_int,C.c_void_p,C.c_int]
x.XCloseDisplay.argtypes = [C.c_void_p]
xt.XTestFakeKeyEvent.argtypes = [C.c_void_p,C.c_uint,C.c_int,C.c_ulong]
xt.XTestFakeMotionEvent.argtypes = [C.c_void_p,C.c_int,C.c_int,C.c_int,C.c_ulong]
xt.XTestFakeButtonEvent.argtypes = [C.c_void_p,C.c_uint,C.c_int,C.c_ulong]
x.XTranslateCoordinates.argtypes = [C.c_void_p,C.c_ulong,C.c_ulong,C.c_int,C.c_int,C.POINTER(C.c_int),C.POINTER(C.c_int),C.POINTER(C.c_ulong)]
x.XGetGeometry.argtypes = [C.c_void_p,C.c_ulong,C.POINTER(C.c_ulong),C.POINTER(C.c_int),C.POINTER(C.c_int),C.POINTER(C.c_uint),C.POINTER(C.c_uint),C.POINTER(C.c_uint),C.POINTER(C.c_uint)]
class Data(C.Union):
    _fields_ = [('l',C.c_long*5),('b',C.c_char*20)]
class Client(C.Structure):
    _fields_ = [('type',C.c_int),('serial',C.c_ulong),('send_event',C.c_int),('display',C.c_void_p),('window',C.c_ulong),('message_type',C.c_ulong),('format',C.c_int),('data',Data)]
class Event(C.Union):
    _fields_ = [('client',Client),('pad',C.c_long*24)]
x.XSendEvent.argtypes = [C.c_void_p,C.c_ulong,C.c_int,C.c_long,C.POINTER(Event)]

def find_window(display):
    root = C.c_ulong(); parent = C.c_ulong(); children = C.POINTER(C.c_ulong)(); count = C.c_uint()
    x.XQueryTree(display,x.XDefaultRootWindow(display),C.byref(root),C.byref(parent),C.byref(children),C.byref(count))
    found = None
    for i in range(count.value):
        name = C.c_char_p()
        if x.XFetchName(display,children[i],C.byref(name)) and name.value:
            if b'capra' in name.value:
                found = children[i]
            x.XFree(name)
    if children:
        x.XFree(children)
    return found

def key(display,window):
    x.XSetInputFocus(display,window,2,0)
    code = x.XKeysymToKeycode(display,x.XStringToKeysym(b'F9'))
    xt.XTestFakeKeyEvent(display,code,1,0); xt.XTestFakeKeyEvent(display,code,0,0)
    x.XFlush(display)

def geometry(display,window):
    root=C.c_ulong(); rx=C.c_int(); ry=C.c_int(); width=C.c_uint(); height=C.c_uint(); border=C.c_uint(); depth=C.c_uint(); child=C.c_ulong()
    x.XGetGeometry(display,window,C.byref(root),C.byref(rx),C.byref(ry),C.byref(width),C.byref(height),C.byref(border),C.byref(depth))
    x.XTranslateCoordinates(display,window,root,0,0,C.byref(rx),C.byref(ry),C.byref(child))
    return rx.value,ry.value,width.value,height.value

def click(display,window,rx,ry):
    ox,oy,_,_=geometry(display,window)
    xt.XTestFakeMotionEvent(display,-1,ox+rx,oy+ry,0)
    xt.XTestFakeButtonEvent(display,1,1,0);xt.XTestFakeButtonEvent(display,1,0,0);x.XFlush(display);time.sleep(.25)

def press(display,symbol,control=False):
    desired=ord(symbol) if len(symbol)==1 else x.XStringToKeysym(symbol.encode())
    code=x.XKeysymToKeycode(display,desired)
    shift=x.XKeysymToKeycode(display,x.XStringToKeysym(b'Shift_L'))
    shifted=x.XkbKeycodeToKeysym(display,code,0,0)!=desired
    ctrl=x.XKeysymToKeycode(display,x.XStringToKeysym(b'Control_L'))
    if control:xt.XTestFakeKeyEvent(display,ctrl,1,0)
    if shifted:xt.XTestFakeKeyEvent(display,shift,1,0)
    xt.XTestFakeKeyEvent(display,code,1,0);xt.XTestFakeKeyEvent(display,code,0,0)
    if shifted:xt.XTestFakeKeyEvent(display,shift,0,0)
    if control:xt.XTestFakeKeyEvent(display,ctrl,0,0)
    x.XFlush(display)

def capture(display_name,label):
    destination=os.environ.get('CAPRA_CAPTURE_DIR')
    if not destination:return
    folder=Path(destination);folder.mkdir(parents=True,exist_ok=True)
    raw=folder/(label+'.xwd');png=folder/(label+'.png')
    S.run(['xwd','-display',display_name,'-root','-silent','-out',str(raw)],check=True)
    target=find_window(display)
    if target:
        ox,oy,width,height=geometry(display,target)
        S.run(['convert',str(raw),'-crop',f'{width}x{height}+{ox}+{oy}','+repage',str(png)],check=True)
    else:
        S.run(['convert',str(raw),str(png)],check=True)
    raw.unlink()

def close(display,window):
    event = Event(); event.client.type = 33; event.client.send_event = 1
    event.client.display = display; event.client.window = window
    event.client.message_type = x.XInternAtom(display,b'WM_PROTOCOLS',0); event.client.format = 32
    event.client.data.l[0] = x.XInternAtom(display,b'WM_DELETE_WINDOW',0)
    x.XSendEvent(display,window,0,0,C.byref(event)); x.XFlush(display)

xvfb = S.Popen(['Xvfb','-displayfd','1','-screen','0','1280x960x24','-nolisten','tcp'],stdout=S.PIPE,stderr=S.DEVNULL)
display_name = ':' + xvfb.stdout.readline().decode().strip()
display = x.XOpenDisplay(display_name.encode())
assert display
compositor=None
if os.environ.get('CAPRA_COMPOSITOR'):
    compositor=S.Popen([os.environ['CAPRA_COMPOSITOR'],'-d',display_name],stdout=S.DEVNULL,stderr=S.DEVNULL)
    time.sleep(.5)
    # The isolated display has no WM. Advertise its client-frame capabilities
    # so GPUI exercises real client-side controls instead of its SSD fallback.
    root=x.XDefaultRootWindow(display)
    owner=(C.c_ulong*1)(root)
    x.XChangeProperty(display,root,x.XInternAtom(display,b'_NET_WM_CM_OWNER',0),x.XInternAtom(display,b'WINDOW',0),32,0,owner,1)
    supported=(C.c_ulong*1)(x.XInternAtom(display,b'_GTK_FRAME_EXTENTS',0))
    x.XChangeProperty(display,root,x.XInternAtom(display,b'_NET_SUPPORTED',0),x.XInternAtom(display,b'ATOM',0),32,0,supported,1)
    x.XFlush(display)
try:
    for keep,stop_first in [(False,True),(True,False)]:
        with tempfile.TemporaryDirectory(prefix='recorder-gui-') as tmp:
            folder = Path(tmp); config = folder/'config/capra'; config.mkdir(parents=True)
            videos = folder/'videos'; videos.mkdir()
            (config/'settings.json').write_text(json.dumps({'video_backend':'X11','record_system':False,'record_mic':False,'save_directory':str(videos),'keep_mkv':keep}))
            env = dict(os.environ,DISPLAY=display_name,XDG_CONFIG_HOME=str(folder/'config'),XDG_SESSION_TYPE='x11',WINIT_UNIX_BACKEND='x11')
            env.pop('WAYLAND_DISPLAY',None)
            app = S.Popen([binary],env=env,stdout=S.PIPE,stderr=S.PIPE)
            try:
                deadline = time.monotonic()+15; window = None
                while time.monotonic()<deadline:
                    window = find_window(display)
                    if window: break
                    if app.poll() is not None: raise RuntimeError(app.communicate()[1].decode())
                    time.sleep(.1)
                assert window, 'GUI did not open'
                time.sleep(1.5)
                capture(display_name,'recorder')
                if os.environ.get('CAPRA_CAPTURE_DIR'):
                    inset=54 if compositor is not None else 0
                    nav_x=130 if compositor is not None else 110
                    click(display,window,nav_x,210+inset);capture(display_name,'recovery')
                    if keep:
                        click(display,window,nav_x,264+inset);capture(display_name,'settings')
                    click(display,window,nav_x,157+inset)
                if os.environ.get('CAPRA_NAVIGATION_TEST') and not keep:
                    inset=54 if compositor is not None else 0
                    click(display,window,130 if compositor is not None else 110,264+inset)
                    capture(display_name,'settings')
                    # Exercise the actual Component switch and persisted retention value.
                    _,_,width,_=geometry(display,window)
                    switch_x=width-(98 if compositor is not None else 78)
                    click(display,window,switch_x,289+inset)
                    assert json.loads((config/'settings.json').read_text())['keep_mkv'] != keep
                    click(display,window,switch_x,289+inset)
                    assert json.loads((config/'settings.json').read_text())['keep_mkv'] == keep
                    focused=False
                    for input_y in range(540,671,10):
                        click(display,window,450,input_y+inset)
                        press(display,'a',True);press(display,'BackSpace');time.sleep(.2)
                        if json.loads((config/'settings.json').read_text())['save_directory']=='':
                            focused=True;break
                    if not focused:capture(display_name,'settings-input-missing')
                    assert focused, 'Directory input was not focused'
                    for char in str(videos):press(display,char)
                    time.sleep(.3)
                    assert json.loads((config/'settings.json').read_text())['save_directory']==str(videos), json.loads((config/'settings.json').read_text())['save_directory']
                    click(display,window,130 if compositor is not None else 110,157+inset)
                def record_control():
                    if os.environ.get('CAPRA_MOUSE_RECORD_TEST'):
                        _,_,width,_=geometry(display,window)
                        inset=54 if compositor is not None else 0
                        click(display,window,width-(127 if compositor is not None else 107),242+inset)
                    else:
                        key(display,window)
                record_control()
                deadline = time.monotonic()+10
                while time.monotonic()<deadline:
                    if any(p.stat().st_size>10000 for p in videos.glob('*.mkv')): break
                    time.sleep(.1)
                else: raise RuntimeError('No recording data was written')
                time.sleep(.6)
                if os.environ.get('CAPRA_MOUSE_RECORD_TEST'):
                    capture(display_name,'recording')
                if stop_first: record_control()
                if stop_first and os.environ.get('CAPRA_MOUSE_RECORD_TEST'):
                    # Prove the square stops and saves before any window-close event.
                    deadline=time.monotonic()+12
                    while not list(videos.glob('*.mp4')):
                        assert app.poll() is None, 'Window closed when stop was clicked'
                        if time.monotonic()>deadline:
                            raise RuntimeError('Clicking the square did not finish the recording')
                        time.sleep(.1)
                if os.environ.get('CAPRA_COMPOSITOR'):
                    _,_,width,_=geometry(display,window)
                    click(display,window,width-37,37)
                else:
                    close(display,window)
                app.wait(timeout=12)
                assert app.returncode==0,app.communicate()[1].decode()
                files = list(videos.glob('*.mp4')); assert len(files)==1, list(videos.iterdir())
                assert bool(list(videos.glob('*.mkv')))==keep
                data = json.loads(S.check_output(['ffprobe','-v','error','-show_entries','format=duration','-of','json',str(files[0])]))
                assert float(data['format']['duration'])>0
                S.run(['ffmpeg','-nostdin','-v','error','-xerror','-i',str(files[0]),'-f','null','-'],check=True)
                print(f'PASS actual GUI stop_first={stop_first}, keep_mkv={keep}',flush=True)
            finally:
                if app.poll() is None: app.terminate(); app.wait(timeout=5)
finally:
    if compositor is not None:
        compositor.terminate();compositor.wait(timeout=5)
    x.XCloseDisplay(display); xvfb.terminate(); xvfb.wait(timeout=5)
