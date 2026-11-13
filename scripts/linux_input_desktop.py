"""Real native input reproduction. All GUI input stays inside a private Xvfb.

Editors write their own Gtk.TextBuffer contents to JSON. This verification does
not use computer-use-rs accessibility text or screenshot capture.
"""
import ctypes
import json
import os
from pathlib import Path
import secrets
import signal
import select
import shutil
import struct
import subprocess
import sys
import time


def wait_for(predicate, description, timeout=15):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(0.05)
    raise RuntimeError("Timed out waiting for " + description)


def editor(role, case, output):
    import gi
    gi.require_version("Gtk", "3.0")
    gi.require_version("GdkX11", "3.0")
    from gi.repository import Gtk, GLib
    GLib.set_prgname("cu-scratch-" + role.lower())
    win = Gtk.Window(title="Scratch editor " + role)
    win.set_default_size(570, 340)
    win.move(20 if role == "A" else 610, 70)
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=16)
    box.set_border_width(20)
    heading = Gtk.Label()
    heading.set_markup("<big><b>" + case + " — Editor " + role + "</b></big>")
    box.pack_start(heading, False, False, 0)
    label = "REQUESTED TARGET (kept unfocused)" if role == "A" else "ACTUAL KEYBOARD FOCUS"
    box.pack_start(Gtk.Label(label=label), False, False, 0)
    view = Gtk.TextView()
    view.set_wrap_mode(Gtk.WrapMode.WORD_CHAR)
    view.set_top_margin(15)
    view.set_left_margin(15)
    box.pack_start(view, True, True, 0)
    box.pack_start(Gtk.Label(label="Harmless scratch buffer; no shell or saved documents"), False, False, 0)
    win.add(box)
    win.connect("destroy", Gtk.main_quit)
    win.show_all()
    view.grab_focus()
    buf = view.get_buffer()

    def snapshot(*_args):
        data = {"role": role, "pid": os.getpid(), "xid": win.get_window().get_xid(),
                "text": buf.get_text(buf.get_start_iter(), buf.get_end_iter(), True)}
        tmp = Path(str(output) + ".tmp")
        tmp.write_text(json.dumps(data))
        tmp.replace(output)
    buf.connect("changed", snapshot)
    snapshot()
    Gtk.main()


def read_json(path):
    try:
        return json.loads(path.read_text())
    except (FileNotFoundError, json.JSONDecodeError):
        return None


def main(out, baseline, fixed):
    import gi
    gi.require_version("Gio", "2.0")
    from gi.repository import Gio, GLib
    from PIL import Image
    out.mkdir(exist_ok=True)
    runtime = out / "runtime"
    runtime.mkdir(mode=0o700)
    home = out / "home"
    home.mkdir()
    # Do not pass desktop addresses, toolkit settings, or credentials from the host.
    env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "LANG": "C.UTF-8",
           "HOME": str(home), "XDG_RUNTIME_DIR": str(runtime), "GDK_BACKEND": "x11",
           "GSETTINGS_BACKEND": "memory", "NO_AT_BRIDGE": "0", "CU_ISOLATED_DESKTOP": "1"}
    auth = out / "Xauthority"
    cookie = secrets.token_bytes(16)
    fields = [b"", b"", b"MIT-MAGIC-COOKIE-1", cookie]
    auth.write_bytes(struct.pack(">H", 65535) + b"".join(struct.pack(">H", len(f)) + f for f in fields))
    auth.chmod(0o600)
    env["XAUTHORITY"] = str(auth)
    processes = []
    frames = []
    results = []
    logs = []

    def spawn(args, name, stdout=None, pass_fds=()):
        log = open(out / (name + ".log"), "w")
        logs.append(log)
        proc = subprocess.Popen(args, env=env, stdin=subprocess.DEVNULL,
                                stdout=stdout if stdout is not None else log,
                                stderr=log, pass_fds=pass_fds, start_new_session=True)
        processes.append(proc)
        return proc

    try:
        rfd, wfd = os.pipe()
        server = spawn([shutil.which("Xvfb"), "-displayfd", str(wfd), "-screen", "0", "1200x480x24",
                        "-nolisten", "tcp", "-auth", str(auth)], "xvfb", pass_fds=(wfd,))
        os.close(wfd)
        with os.fdopen(rfd) as display_pipe:
            if not select.select([display_pipe], [], [], 15)[0]:
                raise RuntimeError("Xvfb failed to announce a display")
            number = display_pipe.readline().strip()
        if not number.isdigit() or server.poll() is not None:
            raise RuntimeError("Xvfb failed to start")
        env["DISPLAY"] = ":" + number
        bus = spawn(["dbus-daemon", "--session", "--nofork", "--nopidfile", "--print-address=1"],
                    "session-bus", stdout=subprocess.PIPE)
        if not select.select([bus.stdout], [], [], 15)[0]:
            raise RuntimeError("private session bus did not start")
        address = bus.stdout.readline().decode().strip()
        env["DBUS_SESSION_BUS_ADDRESS"] = address
        conn = Gio.DBusConnection.new_for_address_sync(
            address, Gio.DBusConnectionFlags.AUTHENTICATION_CLIENT | Gio.DBusConnectionFlags.MESSAGE_BUS_CONNECTION,
            None, None)
        reply = conn.call_sync("org.a11y.Bus", "/org/a11y/bus", "org.a11y.Bus", "GetAddress",
                               None, GLib.VariantType.new("(s)"), Gio.DBusCallFlags.NONE, 10000, None)
        env["AT_SPI_BUS_ADDRESS"] = reply.unpack()[0]
        conn.call_sync("org.a11y.Bus", "/org/a11y/bus", "org.freedesktop.DBus.Properties", "Set",
                       GLib.Variant("(ssv)", ("org.a11y.Status", "IsEnabled", GLib.Variant("b", True))),
                       None, Gio.DBusCallFlags.NONE, 10000, None)
        # Import GDK only after establishing the private display environment.
        os.environ.update(env)
        for key in ["WAYLAND_DISPLAY", "DBUS_STARTER_ADDRESS", "DBUS_STARTER_BUS_TYPE"]:
            os.environ.pop(key, None)
        gi.require_version("Gdk", "3.0")
        gi.require_version("Gtk", "3.0")
        from gi.repository import Gtk, Gdk
        Gtk.init([])
        root = Gdk.get_default_root_window()
        x11 = ctypes.CDLL("libX11.so.6")
        x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
        x11.XOpenDisplay.restype = ctypes.c_void_p
        x11.XSetInputFocus.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_ulong]
        x11.XGetInputFocus.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_ulong), ctypes.POINTER(ctypes.c_int)]
        x11.XSync.argtypes = [ctypes.c_void_p, ctypes.c_int]
        x11.XCloseDisplay.argtypes = [ctypes.c_void_p]
        display = x11.XOpenDisplay(env["DISPLAY"].encode())
        if not display:
            raise RuntimeError("cannot open private X display")

        def focus_id():
            xid, revert = ctypes.c_ulong(), ctypes.c_int()
            x11.XGetInputFocus(display, ctypes.byref(xid), ctypes.byref(revert))
            return xid.value

        def capture():
            while Gtk.events_pending():
                Gtk.main_iteration_do(False)
            pixbuf = Gdk.pixbuf_get_from_window(root, 0, 0, 1200, 480)
            frame = Image.frombytes("RGB", (pixbuf.get_width(), pixbuf.get_height()),
                                    bytes(pixbuf.get_pixels()), "raw", "RGB", pixbuf.get_rowstride())
            frames.append(frame)
            return frame

        cases = [("baseline-background", baseline, "background", "OK", True),
                 ("fixed-background", fixed, "background", "UNSUPPORTED", False),
                 ("baseline-global", baseline, "global", "OK", True),
                 ("fixed-global", fixed, "global", "OK", True)]
        for case, binary, mode, expected_result, expect_in_b in cases:
            paths = {role: out / (case + "-" + role + ".json") for role in ["A", "B"]}
            editors = [spawn([sys.executable, __file__, "--editor", role, case, str(paths[role])],
                             case + "-" + role) for role in ["A", "B"]]
            a, b = [wait_for(lambda p=p: read_json(p), "editor buffer") for p in paths.values()]
            # Wait for AT-SPI registration and focus only this virtual display.
            time.sleep(1)
            x11.XSetInputFocus(display, b["xid"], 2, 0)
            x11.XSync(display, 0)
            wait_for(lambda: focus_id() == b["xid"], "editor B keyboard focus")
            for _ in range(8):
                capture()
                time.sleep(0.1)
            marker = "CU_" + case.replace("-", "_").upper() + "_MARKER"
            driver = spawn([binary, str(a["pid"]), mode, marker], case + "-driver")
            deadline = time.monotonic() + 20
            while driver.poll() is None and time.monotonic() < deadline:
                capture()
                time.sleep(0.1)
            if driver.poll() is None:
                raise RuntimeError("public API driver timed out")
            for _ in range(15):
                capture()
                time.sleep(0.1)
            driver_log = (out / (case + "-driver.log")).read_text()
            after_a, after_b = [read_json(p) for p in paths.values()]
            assert driver.returncode == 0, driver_log
            assert "RESULT=" + expected_result in driver_log, driver_log
            assert after_a["text"] == "", after_a
            assert after_b["text"] == (marker if expect_in_b else ""), after_b
            assert focus_id() == b["xid"], "focus changed during reproduction"
            capture().save(out / (case + ".png"))
            result = {"case": case, "requested_target": "A", "keyboard_focus_before_and_after": "B",
                      "api_result": expected_result, "editor_A_text": after_a["text"],
                      "editor_B_text": after_b["text"], "assertions": "PASS"}
            results.append(result)
            print(json.dumps(result), flush=True)
            for proc in editors:
                proc.terminate()
                proc.wait(timeout=5)
        x11.XCloseDisplay(display)
        (out / "results.json").write_text(json.dumps(results, indent=2) + "\n")
        frames[0].save(out / "desktop-recording.gif", save_all=True, append_images=frames[1:],
                       duration=100, loop=0, optimize=True)
        print("All four real desktop scenarios passed. Evidence:", out, flush=True)
    finally:
        for proc in reversed(processes):
            # Also stop D-Bus-activated registry/bus children, not just their parent.
            try:
                os.killpg(proc.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            if proc.poll() is None:
                try:
                    proc.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    proc.kill()
                    proc.wait()
        for log in logs:
            log.close()


if __name__ == "__main__":
    if sys.argv[1] == "--editor":
        editor(sys.argv[2], sys.argv[3], Path(sys.argv[4]))
    else:
        main(Path(sys.argv[1]), sys.argv[2], sys.argv[3])
