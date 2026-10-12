#requires -Version 7.3
# VM:     ./verify-desktop.ps1 -Zolo path/to/zolo.exe
# Build:  ./verify-desktop.ps1 -Zolo path/to/zolo.exe -Backend native
# Reuse:  ./verify-desktop.ps1 -Backend llvm -Executable path/to/desktop_smoke.exe
# Unicode SendInput exercises UTF16 editing; this does not simulate OS IME composition.
param(
    [string] $Zolo = 'zolo',
    [ValidateSet('vm', 'native', 'llvm')] [string] $Backend = 'vm',
    [string] $Executable = '',
    [string] $Library = '',
    [int] $StartupTimeoutSeconds = 900,
    [int] $StepTimeoutSeconds = 20
)

$ErrorActionPreference = 'Stop'
if (!$IsWindows) { throw 'This gate requires an interactive Windows desktop.' }
$library = if ($Library) { (Resolve-Path -LiteralPath $Library).Path } else { Split-Path $PSScriptRoot -Parent }
$fixture = Join-Path $library 'examples/desktop_smoke.zolo'
$runId = [Guid]::NewGuid().ToString('N')
$runDir = Join-Path $library "target/desktop-verification/$Backend/$runId"
[IO.Directory]::CreateDirectory($runDir) | Out-Null
$taskUtf8 = [Text.UTF8Encoding]::new($false)
$taskHistory = [Collections.Generic.List[string]]::new()
$taskProcess = $null

if (!('DesktopWindowReceipt' -as [type])) {
    Add-Type @'
using System;
using System.Collections.Concurrent;
using System.Collections.Generic;
using System.ComponentModel;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;

public static class DesktopWindowReceipt {
    public static readonly ConcurrentQueue<string> Lines = new ConcurrentQueue<string>();
    [StructLayout(LayoutKind.Sequential)] public struct Point { public int X, Y; }
    [StructLayout(LayoutKind.Sequential)] struct Rect { public int Left, Top, Right, Bottom; }
    [StructLayout(LayoutKind.Sequential)] struct MouseInput {
        public int X, Y; public uint Data, Flags, Time; public UIntPtr Extra;
    }
    [StructLayout(LayoutKind.Sequential)] struct KeyboardInput {
        public ushort Key, Scan; public uint Flags, Time; public UIntPtr Extra;
    }
    [StructLayout(LayoutKind.Explicit)] struct InputUnion {
        [FieldOffset(0)] public MouseInput Mouse;
        [FieldOffset(0)] public KeyboardInput Keyboard;
    }
    [StructLayout(LayoutKind.Sequential)] struct Input { public uint Type; public InputUnion Data; }
    delegate bool EnumWindowsCallback(IntPtr window, IntPtr parameter);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumWindowsCallback callback, IntPtr parameter);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr window);
    [DllImport("user32.dll")] static extern bool IsWindow(IntPtr window);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr window, out uint process);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetWindowText(IntPtr window, StringBuilder text, int count);
    [DllImport("user32.dll")] static extern bool GetClientRect(IntPtr window, out Rect rect);
    [DllImport("user32.dll")] static extern bool ClientToScreen(IntPtr window, ref Point point);
    [DllImport("user32.dll")] static extern uint GetDpiForWindow(IntPtr window);
    [DllImport("user32.dll")] static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] static extern bool SetForegroundWindow(IntPtr window);
    [DllImport("user32.dll")] static extern bool BringWindowToTop(IntPtr window);
    [DllImport("user32.dll")] static extern bool IsIconic(IntPtr window);
    [DllImport("user32.dll")] static extern bool ShowWindow(IntPtr window, int command);
    [DllImport("user32.dll")] static extern bool AttachThreadInput(uint thread, uint other, bool attach);
    [DllImport("kernel32.dll")] static extern uint GetCurrentThreadId();
    [DllImport("user32.dll", EntryPoint="GetPhysicalCursorPos")] public static extern bool GetCursorPos(out Point point);
    [DllImport("user32.dll", EntryPoint="SetPhysicalCursorPos")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll", SetLastError=true)] static extern uint SendInput(uint count, Input[] inputs, int size);
    [DllImport("user32.dll", SetLastError=true)] static extern bool PostMessage(IntPtr window, uint message, UIntPtr w, IntPtr l);

    [DllImport("user32.dll")] static extern IntPtr GetDC(IntPtr window);
    [DllImport("user32.dll")] static extern int ReleaseDC(IntPtr window, IntPtr dc);
    [DllImport("user32.dll")] static extern bool PrintWindow(IntPtr window, IntPtr dc, uint flags);
    [DllImport("gdi32.dll")] static extern IntPtr CreateCompatibleDC(IntPtr dc);
    [DllImport("gdi32.dll")] static extern IntPtr CreateCompatibleBitmap(IntPtr dc, int width, int height);
    [DllImport("gdi32.dll")] static extern IntPtr SelectObject(IntPtr dc, IntPtr value);
    [DllImport("gdi32.dll")] public static extern bool DeleteObject(IntPtr value);
    [DllImport("gdi32.dll")] static extern bool DeleteDC(IntPtr dc);
    [DllImport("gdi32.dll")] static extern bool BitBlt(IntPtr destination, int x, int y, int width, int height, IntPtr source, int sx, int sy, uint operation);

    public static void Capture(Process process) {
        process.OutputDataReceived += (_, e) => { if (e.Data != null) Lines.Enqueue(e.Data); };
        process.ErrorDataReceived += (_, e) => { if (e.Data != null) Lines.Enqueue(e.Data); };
        process.BeginOutputReadLine(); process.BeginErrorReadLine();
    }
    public static IntPtr[] Windows(int process) {
        var windows = new List<IntPtr>();
        EnumWindows((window, _) => {
            uint owner; GetWindowThreadProcessId(window, out owner);
            if (owner == process && IsWindowVisible(window) && Title(window).StartsWith("Zolo desktop verification ")) windows.Add(window);
            return true;
        }, IntPtr.Zero);
        return windows.ToArray();
    }
    public static string Title(IntPtr window) {
        var title = new StringBuilder(256); GetWindowText(window, title, title.Capacity); return title.ToString();
    }
    public static void VerifyOwner(IntPtr window, int process) {
        uint owner; GetWindowThreadProcessId(window, out owner);
        if (!IsWindow(window) || owner != process) throw new InvalidOperationException("Target window is not owned by the child process");
    }
    static void Foreground(IntPtr window) {
        var previous = GetForegroundWindow();
        uint ignored; var previousThread = GetWindowThreadProcessId(previous, out ignored);
        var ownedThread = GetWindowThreadProcessId(window, out ignored);
        var callingThread = GetCurrentThreadId();
        bool attachedPrevious = previousThread != 0 && previousThread != callingThread && AttachThreadInput(callingThread, previousThread, true);
        bool attachedOwned = ownedThread != 0 && ownedThread != callingThread && ownedThread != previousThread && AttachThreadInput(callingThread, ownedThread, true);
        try {
            if (IsIconic(window)) ShowWindow(window, 9);
            BringWindowToTop(window); SetForegroundWindow(window);
        } finally {
            if (attachedOwned) AttachThreadInput(callingThread, ownedThread, false);
            if (attachedPrevious) AttachThreadInput(callingThread, previousThread, false);
        }
    }
    public static void Activate(IntPtr window, int process) {
        VerifyOwner(window, process); Foreground(window); Thread.Sleep(60);
        if (GetForegroundWindow() != window) throw new InvalidOperationException("Windows did not grant focus to the owned child; no input was sent");
    }
    static void VerifyInputTarget(IntPtr window, int process) {
        VerifyOwner(window, process);
        if (GetForegroundWindow() != window) throw new InvalidOperationException("Owned window lost foreground focus; no input was sent");
    }
    static void Inject(IntPtr window, int process, params Input[] inputs) {
        VerifyInputTarget(window, process);
        if (SendInput((uint)inputs.Length, inputs, Marshal.SizeOf(typeof(Input))) != inputs.Length) throw new Win32Exception(Marshal.GetLastWin32Error(), "SendInput did not inject all owned-window input");
    }
    static Input Key(ushort key, bool up) {
        return new Input { Type = 1, Data = new InputUnion { Keyboard = new KeyboardInput { Key = key, Flags = up ? 2u : 0u } } };
    }
    static Input Mouse(bool up) {
        return new Input { Type = 0, Data = new InputUnion { Mouse = new MouseInput { Flags = up ? 4u : 2u } } };
    }
    static Point ClientPoint(IntPtr window, double x, double y) {
        var previous = SetThreadDpiAwarenessContext(new IntPtr(-4));
        try {
            var dpi = GetDpiForWindow(window); double scale = dpi == 0 ? 1 : dpi / 96.0;
            var point = new Point { X = (int)Math.Round(x * scale), Y = (int)Math.Round(y * scale) };
            Rect rect; if (!GetClientRect(window, out rect) || point.X < 0 || point.Y < 0 || point.X >= rect.Right || point.Y >= rect.Bottom) throw new InvalidOperationException("Fixture control is outside the owned client area");
            if (!ClientToScreen(window, ref point)) throw new Win32Exception();
            return point;
        } finally { if (previous != IntPtr.Zero) SetThreadDpiAwarenessContext(previous); }
    }
    public static void Click(IntPtr window, int process, double x, double y) {
        Activate(window, process); var point = ClientPoint(window, x, y);
        SetCursorPos(point.X, point.Y); Thread.Sleep(30); Inject(window, process, Mouse(false), Mouse(true));
    }
    public static void Drag(IntPtr window, int process, double x1, double y1, double x2, double y2) {
        Activate(window, process); var first = ClientPoint(window, x1, y1); var last = ClientPoint(window, x2, y2);
        SetCursorPos(first.X, first.Y); Thread.Sleep(40); Inject(window, process, Mouse(false));
        try {
            for (int step = 1; step <= 8; step++) {
                VerifyInputTarget(window, process);
                SetCursorPos(first.X + (last.X-first.X)*step/8, first.Y + (last.Y-first.Y)*step/8); Thread.Sleep(30);
            }
        } finally { Activate(window, process); Inject(window, process, Mouse(true)); }
    }
    public static void Chord(IntPtr window, int process, ushort key, bool control, bool shift) {
        VerifyInputTarget(window, process); var inputs = new List<Input>();
        if (control) inputs.Add(Key(0x11, false)); if (shift) inputs.Add(Key(0x10, false));
        inputs.Add(Key(key, false)); inputs.Add(Key(key, true));
        if (shift) inputs.Add(Key(0x10, true)); if (control) inputs.Add(Key(0x11, true));
        Inject(window, process, inputs.ToArray());
    }
    public static void Text(IntPtr window, int process, string text) {
        foreach (var unit in text) {
            var down = new Input { Type = 1, Data = new InputUnion { Keyboard = new KeyboardInput { Scan = unit, Flags = 4 } } };
            var up = down; up.Data.Keyboard.Flags = 6;
            Inject(window, process, down, up); Thread.Sleep(25);
        }
    }
    public static void CloseOwned(IntPtr window, int process) {
        VerifyOwner(window, process); PostMessage(window, 0x10, UIntPtr.Zero, IntPtr.Zero);
    }
    // Neither path reads a desktop/screen DC. Both are restricted to the
    // verified child HWND's client rectangle, excluding unrelated windows.
    public static IntPtr CaptureBitmap(IntPtr window, int process, bool printWindow) {
        Activate(window, process);
        var previous = SetThreadDpiAwarenessContext(new IntPtr(-4));
        IntPtr source = IntPtr.Zero, destination = IntPtr.Zero, bitmap = IntPtr.Zero, old = IntPtr.Zero;
        try {
            VerifyInputTarget(window, process);
            Rect client; if (!GetClientRect(window, out client)) throw new Win32Exception();
            source = GetDC(window); destination = CreateCompatibleDC(source);
            bitmap = CreateCompatibleBitmap(source, client.Right, client.Bottom);
            if (source == IntPtr.Zero || destination == IntPtr.Zero || bitmap == IntPtr.Zero) throw new Win32Exception();
            old = SelectObject(destination, bitmap);
            bool captured = printWindow ? PrintWindow(window, destination, 3u)
                : BitBlt(destination, 0, 0, client.Right, client.Bottom, source, 0, 0, 0x00cc0020u);
            if (!captured) throw new Win32Exception("Could not capture the owned child window");
            var result = bitmap; bitmap = IntPtr.Zero; return result;
        } finally {
            if (old != IntPtr.Zero && destination != IntPtr.Zero) SelectObject(destination, old);
            if (bitmap != IntPtr.Zero) DeleteObject(bitmap);
            if (destination != IntPtr.Zero) DeleteDC(destination);
            if (source != IntPtr.Zero) ReleaseDC(window, source);
            if (previous != IntPtr.Zero) SetThreadDpiAwarenessContext(previous);
        }
    }
    public static void RestoreForeground(IntPtr window) { if (window != IntPtr.Zero && IsWindow(window)) Foreground(window); }
}
'@
}
[DesktopWindowReceipt]::Lines.Clear()
$previousForeground = [DesktopWindowReceipt]::GetForegroundWindow()
$previousPointer = [DesktopWindowReceipt+Point]::new()
[DesktopWindowReceipt]::GetCursorPos([ref] $previousPointer) | Out-Null

function Drain-Receipts {
    $line = ''
    while ([DesktopWindowReceipt]::Lines.TryDequeue([ref] $line)) {
        $taskHistory.Add($line)
        Write-Host $line
    }
}
function Assert-Live {
    if ($taskProcess.HasExited) { Drain-Receipts; throw "Owned child exited $($taskProcess.ExitCode) before the UI gate completed." }
}
function Wait-Receipt([string] $needle, [int] $timeout = $StepTimeoutSeconds) {
    $timer = [Diagnostics.Stopwatch]::StartNew()
    while ($timer.Elapsed.TotalSeconds -lt $timeout) {
        Drain-Receipts
        if ($taskHistory.Contains($needle)) { return }
        if ($taskProcess.HasExited) {
            $taskProcess.WaitForExit()
            Drain-Receipts
            if ($taskHistory.Contains($needle)) { return }
            throw "Owned child exited $($taskProcess.ExitCode) before native receipt: $needle"
        }
        Start-Sleep -Milliseconds 15
    }
    throw "Missing native receipt: $needle"
}
function Save-OwnedScreenshot([IntPtr] $window, [string] $name = 'desktop-startup') {
    Assert-Live
    Add-Type -AssemblyName System.Drawing
    $screenshot = Join-Path $runDir ($name + '.png')
    foreach ($printWindow in @($true, $false)) {
        $handle = [IntPtr]::Zero
        $bitmap = $null
        try {
            $handle = [DesktopWindowReceipt]::CaptureBitmap($window, $taskProcess.Id, $printWindow)
            $bitmap = [Drawing.Bitmap]::FromHbitmap($handle)
            # A GPU window can return an empty image even when PrintWindow
            # succeeds. Require color variation before reporting a screenshot.
            $colors = [Collections.Generic.HashSet[int]]::new()
            for ($y = 12; $y -lt $bitmap.Height; $y += 19) {
                for ($x = 12; $x -lt $bitmap.Width; $x += 19) { $colors.Add($bitmap.GetPixel($x, $y).ToArgb()) | Out-Null }
            }
            if ($colors.Count -lt 4) { continue }
            $bitmap.Save($screenshot, [Drawing.Imaging.ImageFormat]::Png)
            $method = if ($printWindow) { 'owned_client_printwindow' } else { 'owned_client_dc' }
            $receipt = "DESKTOP_SCREENSHOT path=$screenshot method=$method width=$($bitmap.Width) height=$($bitmap.Height)"
            $taskHistory.Add($receipt)
            Write-Host $receipt
            return $screenshot
        } catch {
            if (!$printWindow) { throw }
        } finally {
            if ($bitmap) { $bitmap.Dispose() }
            if ($handle -ne [IntPtr]::Zero) { [DesktopWindowReceipt]::DeleteObject($handle) | Out-Null }
        }
    }
    throw 'Owned GPU window capture contained no UI; no visual screenshot receipt was issued.'
}

function Wait-Window([string] $title) {
    $timer = [Diagnostics.Stopwatch]::StartNew()
    while ($timer.Elapsed.TotalSeconds -lt $StepTimeoutSeconds) {
        Assert-Live
        foreach ($window in [DesktopWindowReceipt]::Windows($taskProcess.Id)) {
            if ([DesktopWindowReceipt]::Title($window) -eq $title) { return $window }
        }
        Drain-Receipts
        Start-Sleep -Milliseconds 20
    }
    throw "Owned native window missing: $title"
}
function Click-Control([IntPtr] $window, [double] $x, [double] $y) {
    Assert-Live
    [DesktopWindowReceipt]::Click($window, $taskProcess.Id, $x, $y)
    # GPUI installs the focused text input handler during its next paint.
    # Synthetic WM_CHAR must follow that paint, as normal user typing does.
    Start-Sleep -Milliseconds 150
}
function Send-Key([IntPtr] $window, [UInt16] $key, [bool] $control = $false, [bool] $shift = $false) {
    Assert-Live
    [DesktopWindowReceipt]::Chord($window, $taskProcess.Id, $key, $control, $shift)
}
function Send-Text([IntPtr] $window, [string] $text) {
    Assert-Live
    [DesktopWindowReceipt]::Text($window, $taskProcess.Id, $text)
}

try {
    if ($Executable) {
        $providedExecutable = (Resolve-Path -LiteralPath $Executable).Path
        $deployment = Join-Path $runDir 'deployment'
        [IO.Directory]::CreateDirectory($deployment) | Out-Null
        $launch = Join-Path $deployment 'desktop_smoke.exe'
        Copy-Item -LiteralPath $providedExecutable -Destination $launch
        $sourceFree = $true
    } else {
        $cli = (Get-Command $Zolo -ErrorAction Stop).Source
        if ($Backend -eq 'vm') { $launch = $cli; $sourceFree = $false }
        else {
            Write-Host "DESKTOP_GATE preparing $Backend embedded executable"
            $buildBase = Join-Path $runDir 'desktop_smoke'
            Push-Location (Join-Path $library 'examples')
            try {
                & $cli build $fixture --emit $Backend --plugins embed -o $buildBase
                if ($LASTEXITCODE -ne 0) { throw "Desktop $Backend build failed ($LASTEXITCODE)." }
            } finally { Pop-Location }
            $deployment = Join-Path $runDir 'deployment'
            [IO.Directory]::CreateDirectory($deployment) | Out-Null
            $launch = Join-Path $deployment 'desktop_smoke.exe'
            Copy-Item -LiteralPath "$buildBase.exe" -Destination $launch
            $sourceFree = $true
        }
    }
    $start = [Diagnostics.ProcessStartInfo]::new($launch)
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $start.StandardOutputEncoding = $taskUtf8
    $start.StandardErrorEncoding = $taskUtf8
    if ($sourceFree) {
        $start.WorkingDirectory = $deployment
        $start.Environment['PATH'] = "$env:SystemRoot/System32;$env:SystemRoot"
        foreach ($name in @('ZOLO_RUSTC', 'ZOLO_CARGO', 'ZOLO_CARGO_EXECUTABLE')) { $start.Environment[$name] = 'missing-compiler-for-desktop-source-free-gate' }
        foreach ($name in @('ZOLO_PLUGIN_DIR', 'ZOLO_STORE_DIR', 'ZOLO_FOREIGN_CACHE', 'ZOLO_FOREIGN_PREPARE_CACHE')) { $start.Environment[$name] = Join-Path $runDir "missing-$name" }
    } else {
        $start.WorkingDirectory = Join-Path $library 'examples'
        $start.ArgumentList.Add('run'); $start.ArgumentList.Add($fixture); $start.ArgumentList.Add('--no-cache')
    }
    $taskProcess = [Diagnostics.Process]::new()
    $taskProcess.StartInfo = $start
    Write-Host "DESKTOP_GATE starting $Backend real-window gate; first Rust preparation may take several minutes"
    $taskProcess.Start() | Out-Null
    [DesktopWindowReceipt]::Capture($taskProcess)
    Wait-Receipt 'DESKTOP:READY:0' $StartupTimeoutSeconds
    Wait-Receipt 'DESKTOP:MAIN:TASK'
    $mainWindow = Wait-Window 'Zolo desktop verification main'
    Start-Sleep -Milliseconds 180

    try { Save-OwnedScreenshot $mainWindow | Out-Null } catch { Write-Host "DESKTOP_SCREENSHOT_UNAVAILABLE $($_.Exception.Message)" }

    Click-Control $mainWindow 70 82
    Send-Text $mainWindow 'x'
    Wait-Receipt 'DESKTOP:INPUT:REJECT:x'
    # Window titles are commands, not a barrier for the next native paint.
    # Wait for the controlled value and focused input handler to be painted.
    if ((Wait-Window 'Zolo desktop verification main rejected') -ne $mainWindow) { throw 'Validation replaced the native window.' }
    Start-Sleep -Milliseconds 150
    Send-Text $mainWindow '7'
    Wait-Receipt 'DESKTOP:INPUT:ACCEPT:7'
    Start-Sleep -Milliseconds 150
    Send-Key $mainWindow 0x41 $true
    Send-Text $mainWindow 'ação🦀日本'
    Wait-Receipt 'DESKTOP:INPUT:ACCEPT:ação🦀日本'
    Start-Sleep -Milliseconds 150
    Send-Key $mainWindow 0x0D
    Wait-Receipt 'DESKTOP:SUBMIT:ação🦀日本'
    Click-Control $mainWindow 70 138
    Wait-Receipt 'DESKTOP:TOGGLE:1:true'
    Send-Key $mainWindow 0x20
    Wait-Receipt 'DESKTOP:TOGGLE:2:false'
    Click-Control $mainWindow 70 194
    Wait-Receipt 'DESKTOP:BUTTON:1'
    Send-Key $mainWindow 0x0D
    Wait-Receipt 'DESKTOP:BUTTON:2'
    Click-Control $mainWindow 70 250
    Wait-Receipt 'DESKTOP:CONTROLS:PASS'
    Wait-Receipt 'DESKTOP:READY:1'
    Start-Sleep -Milliseconds 120

    Click-Control $mainWindow 70 82
    Send-Key $mainWindow 0x0D $true
    Wait-Receipt 'DESKTOP:NESTED_KEY:1'
    Click-Control $mainWindow 70 138
    Wait-Receipt 'DESKTOP:MODAL:OPEN'
    Start-Sleep -Milliseconds 180
    Send-Key $mainWindow 0x0D $true
    Send-Text $mainWindow 'a'
    Wait-Receipt 'DESKTOP:MODAL:A:a'
    Send-Key $mainWindow 0x09 $false $true
    Send-Key $mainWindow 0x20
    Wait-Receipt 'DESKTOP:MODAL:SENTINEL:1'
    Send-Key $mainWindow 0x09
    Send-Text $mainWindow 'A'
    Wait-Receipt 'DESKTOP:MODAL:A:aA'
    Send-Key $mainWindow 0x09
    Send-Text $mainWindow 'b'
    Wait-Receipt 'DESKTOP:MODAL:B:b'
    Send-Key $mainWindow 0x09
    Send-Key $mainWindow 0x20
    Wait-Receipt 'DESKTOP:MODAL:SENTINEL:2'
    Send-Key $mainWindow 0x09
    Send-Text $mainWindow 'C'
    Wait-Receipt 'DESKTOP:MODAL:A:aAC'
    Send-Key $mainWindow 0x1B
    Wait-Receipt 'DESKTOP:MODAL:CLOSED:trap=true:key_isolation=true'
    Start-Sleep -Milliseconds 120
    Click-Control $mainWindow 70 194
    Wait-Receipt 'DESKTOP:KEYBOARD:PASS'
    Wait-Receipt 'DESKTOP:READY:2'
    Start-Sleep -Milliseconds 120
    try { Save-OwnedScreenshot $mainWindow 'desktop-split-before' | Out-Null } catch { Write-Host "DESKTOP_SCREENSHOT_UNAVAILABLE $($_.Exception.Message)" }

    Assert-Live
    [DesktopWindowReceipt]::Drag($mainWindow, $taskProcess.Id, 300, 170, 410, 170)
    Wait-Receipt 'DESKTOP:SPLIT:RESIZED'
    Start-Sleep -Milliseconds 150
    try { Save-OwnedScreenshot $mainWindow 'desktop-split-after' | Out-Null } catch { Write-Host "DESKTOP_SCREENSHOT_UNAVAILABLE $($_.Exception.Message)" }
    Click-Control $mainWindow 70 314
    Wait-Receipt 'DESKTOP:SPLIT:PASS'
    Wait-Receipt 'DESKTOP:READY:3'
    Start-Sleep -Milliseconds 120

    Click-Control $mainWindow 70 82
    Wait-Receipt 'DESKTOP:CHILD:OPEN'
    Wait-Receipt 'DESKTOP:CHILD:TASK'
    $childWindow = Wait-Window 'Zolo desktop verification child'
    if ([DesktopWindowReceipt]::Windows($taskProcess.Id).Length -ne 2) { throw 'Expected two owned native windows.' }
    Click-Control $mainWindow 70 138
    Wait-Receipt 'DESKTOP:MAIN:CLOSING'
    Wait-Receipt 'DESKTOP:MAIN:CLEANUP'
    $timer = [Diagnostics.Stopwatch]::StartNew()
    while ([DesktopWindowReceipt]::Windows($taskProcess.Id).Length -ne 1 -and $timer.Elapsed.TotalSeconds -lt $StepTimeoutSeconds) { Assert-Live; Start-Sleep -Milliseconds 20 }
    if ([DesktopWindowReceipt]::Windows($taskProcess.Id).Length -ne 1) { throw 'First window did not close independently.' }
    Click-Control $childWindow 70 82
    Wait-Receipt 'DESKTOP:CHILD:PING:main_open=false:cleanup=true'
    Click-Control $childWindow 70 138
    Wait-Receipt 'DESKTOP:CHILD:CLOSING'
    Wait-Receipt 'DESKTOP:CHILD:CLEANUP'
    Wait-Receipt 'DESKTOP:PASS:controls:unicode:keyboard:modal:split:multiwindow:cleanup'
    if (!$taskProcess.WaitForExit(15000)) { throw 'Closing the final window did not finish the owned application.' }
    Drain-Receipts
    if ($taskProcess.ExitCode -ne 0) { throw "Desktop child exited $($taskProcess.ExitCode)." }
    $receipt = "DESKTOP_RECEIPT backend=$Backend source_free=$($sourceFree.ToString().ToLowerInvariant()) controls=true controlled_rejection=true unicode_utf16=true nested_shortcut=true modal_shortcut_isolation=true modal_tab_trap=true split_drag=true dynamic_multiwindow=true cleanup=true close=0 physical_ime=not_exercised"
    $taskHistory.Add($receipt)
    Write-Host $receipt
} finally {
    if ($taskProcess -and !$taskProcess.HasExited) {
        foreach ($window in [DesktopWindowReceipt]::Windows($taskProcess.Id)) { [DesktopWindowReceipt]::CloseOwned($window, $taskProcess.Id) }
        if (!$taskProcess.WaitForExit(5000)) { $taskProcess.Kill($true); $taskProcess.WaitForExit() }
    }
    Drain-Receipts
    [IO.File]::WriteAllLines((Join-Path $runDir 'receipt.log'), $taskHistory, $taskUtf8)
    [DesktopWindowReceipt]::SetCursorPos($previousPointer.X, $previousPointer.Y) | Out-Null
    [DesktopWindowReceipt]::RestoreForeground($previousForeground)
    if ($taskProcess) { $taskProcess.Dispose() }
    Write-Host "DESKTOP_GATE receipt: $(Join-Path $runDir 'receipt.log')"
}
