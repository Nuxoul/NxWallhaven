"""Run explicit emulator-only acceptance tests. Build both APKs first.
Requires an existing disposable AVD; never selects or modifies a physical device.
Artifacts default to PI_SCRATCH_DIR, or a fresh system temporary directory.
"""
import argparse
import os
from pathlib import Path
import subprocess
import tempfile
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from threading import Thread


class SessionFixture(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass  # Never log Cookie/header/body values.

    def do_GET(self):
        logged = 'nxrestart=logged' in self.headers.get('Cookie', '')
        if self.path == '/login':
            body = '<form method="post" action="/fixture-login"><button id="signin" type="submit">Local login</button></form>'
        elif logged:
            body = '<div id="userpanel"><a href="/user/RestartFixture">RestartFixture</a><form action="/logout"></form></div>'
        else:
            body = '<div id="userpanel"><a href="/login">Login</a></div>'
        self.reply(body)

    def do_POST(self):
        if self.path != '/fixture-login':
            self.send_error(404)
            return
        self.rfile.read(int(self.headers.get('Content-Length', 0)))
        self.reply('<div id="userpanel"><a href="/user/RestartFixture">RestartFixture</a><form action="/logout"></form></div>', True)

    def reply(self, body, login=False):
        data = body.encode('utf-8')
        self.send_response(200)
        self.send_header('Content-Type', 'text/html; charset=utf-8')
        self.send_header('Content-Length', str(len(data)))
        if login:
            self.send_header('Set-Cookie', 'nxrestart=logged; Max-Age=3600; Path=/; HttpOnly; SameSite=Strict')
        self.end_headers()
        self.wfile.write(data)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--avd', default='NxWallhavenAcceptance')
    parser.add_argument('--port', type=int, default=5580)
    parser.add_argument('--site-only', action='store_true', help='Run only local website-session tests')
    parser.add_argument('--downloads-only', action='store_true', help='Run only download/library tests and restart probes')
    args = parser.parse_args()
    if args.port % 2 or not 5554 <= args.port <= 5682:
        parser.error('port must be even and within 5554..5682')
    root = Path(__file__).resolve().parent.parent
    artifacts = Path(os.environ.get('PI_SCRATCH_DIR') or tempfile.mkdtemp(prefix='nxwh-android-')) / 'android-device-tests'
    artifacts.mkdir(parents=True, exist_ok=True)
    sdk = Path(os.environ.get('ANDROID_HOME') or os.environ['ANDROID_SDK_ROOT'])
    suffix = '.exe' if os.name == 'nt' else ''
    adb = str(sdk / 'platform-tools' / ('adb' + suffix))
    serial = f'emulator-{args.port}'

    def run(command, timeout=90, check=True):
        result = subprocess.run([adb, '-s', serial] + command, capture_output=True,
                                text=True, encoding='utf-8', errors='replace', timeout=timeout)
        if check and result.returncode:
            raise RuntimeError(f'adb {command[:2]}: {result.stdout[-2000:]} {result.stderr[-2000:]}')
        return result

    existing = subprocess.check_output([adb, 'devices'], text=True)
    if serial in existing:
        raise RuntimeError(f'{serial} is already in use; not touching it')
    app = root / 'composeApp/build/outputs/apk/debug/composeApp-debug.apk'
    tests = root / 'composeApp/build/outputs/apk/androidTest/debug/composeApp-debug-androidTest.apk'
    if not app.is_file() or not tests.is_file():
        raise RuntimeError('Build :composeApp:assembleDebug :composeApp:assembleDebugAndroidTest first')
    runner = 'io.github.nuxoul.nxwallhaven.test/androidx.test.runner.AndroidJUnitRunner'

    def instrument(selector, name, expected, extra=None):
        result = run(['shell', 'am', 'instrument', '-w', '-r', '-e', 'restartProbe', 'true'] + (extra or []) +
                     ['-e', 'class', selector, runner], timeout=720, check=False)
        (artifacts / (name + '.txt')).write_text(result.stdout + result.stderr, encoding='utf-8')
        print(result.stdout, flush=True)
        if expected not in result.stdout:
            raise RuntimeError(f'{name} failed; see {artifacts}')

    with (artifacts / 'emulator.log').open('w', encoding='utf-8') as log:
        process = subprocess.Popen([str(sdk / 'emulator' / ('emulator' + suffix)), '-avd', args.avd,
                                    '-port', str(args.port), '-no-snapshot', '-no-window', '-no-audio',
                                    '-no-boot-anim', '-gpu', 'swiftshader_indirect', '-feature', '-Vulkan',
                                    '-memory', '3072', '-cores', '4'], stdout=log, stderr=subprocess.STDOUT)
        try:
            deadline = time.monotonic() + 300
            while time.monotonic() < deadline:
                if process.poll() is not None:
                    raise RuntimeError('Emulator exited before boot')
                if run(['shell', 'getprop', 'sys.boot_completed'], timeout=10, check=False).stdout.strip() == '1':
                    break
                time.sleep(2)
            else:
                raise TimeoutError('Emulator boot did not complete in 300 seconds')
            print('BOOTED ' + serial, flush=True)
            run(['shell', 'input', 'keyevent', '82'])
            print(run(['install', '-r', str(app)], timeout=180).stdout, flush=True)
            print(run(['install', '-r', '-t', str(tests)], timeout=180).stdout, flush=True)
            if not args.site_only and not args.downloads_only:
                instrument('io.github.nuxoul.nxwallhaven.AndroidAcceptanceTest', 'acceptance', 'OK (4 tests)')
            if not args.site_only:
                instrument('io.github.nuxoul.nxwallhaven.DownloadLibraryTest', 'download-library', 'OK (4 tests)')
                instrument('io.github.nuxoul.nxwallhaven.DownloadRestartProbe#prepare', 'download-restart-prepare', 'OK (1 test)')
                run(['shell', 'am', 'force-stop', 'io.github.nuxoul.nxwallhaven'])
                run(['shell', 'svc', 'wifi', 'disable'])
                run(['shell', 'svc', 'data', 'disable'])
                time.sleep(3)
                instrument('io.github.nuxoul.nxwallhaven.DownloadRestartProbe#restoreOfflineAndReconcile', 'download-restart-restore', 'OK (1 test)')
                run(['shell', 'svc', 'wifi', 'enable'])
                run(['shell', 'svc', 'data', 'enable'])
            if args.downloads_only:
                return
            instrument('io.github.nuxoul.nxwallhaven.WebsiteSessionTest', 'website-session', 'OK (4 tests)')
            host = ThreadingHTTPServer(('127.0.0.1', 0), SessionFixture)
            thread = Thread(target=host.serve_forever, daemon=True)
            thread.start()
            port = str(host.server_port)
            try:
                run(['reverse', 'tcp:' + port, 'tcp:' + port])
                instrument('io.github.nuxoul.nxwallhaven.WebSessionRestartProbe#prepare', 'session-restart-prepare', 'OK (1 test)', ['-e', 'sessionPort', port])
                run(['shell', 'am', 'force-stop', 'io.github.nuxoul.nxwallhaven'])
                instrument('io.github.nuxoul.nxwallhaven.WebSessionRestartProbe#restoreAndClear', 'session-restart-restore', 'OK (1 test)', ['-e', 'sessionPort', port])
            finally:
                run(['reverse', '--remove', 'tcp:' + port], check=False)
                host.shutdown()
                host.server_close()
                thread.join(timeout=5)
            if not args.site_only:
                instrument('io.github.nuxoul.nxwallhaven.CacheRestartProbe#prepare', 'restart-prepare', 'OK (1 test)')
                run(['shell', 'am', 'force-stop', 'io.github.nuxoul.nxwallhaven'])
                run(['shell', 'svc', 'wifi', 'disable'])
                run(['shell', 'svc', 'data', 'disable'])
                time.sleep(3)
                instrument('io.github.nuxoul.nxwallhaven.CacheRestartProbe#offlineAfterRestart', 'restart-offline', 'OK (1 test)')
        finally:
            run(['shell', 'svc', 'wifi', 'enable'], check=False)
            run(['shell', 'svc', 'data', 'enable'], check=False)
            run(['pull', '/sdcard/Android/data/io.github.nuxoul.nxwallhaven/files', str(artifacts / 'screens')], check=False)
            (artifacts / 'crash.log').write_text(run(['logcat', '-d', '-b', 'crash'], check=False).stdout, encoding='utf-8')
            run(['emu', 'kill'], check=False)
            try:
                process.wait(timeout=25)
            except subprocess.TimeoutExpired:
                process.terminate()
    print('Artifacts: ' + str(artifacts))


if __name__ == '__main__':
    main()
