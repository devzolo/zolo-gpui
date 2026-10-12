#requires -Version 7.3
param([string]$Zolo = 'zolo', [string]$Library = '')
$ErrorActionPreference = 'Stop'
if (!$IsWindows) { throw 'This gate requires an interactive Windows desktop.' }
if (!$Library) { $Library = Split-Path $PSScriptRoot -Parent }
$libraryRoot = (Resolve-Path -LiteralPath $Library).Path
$examplesRoot = Join-Path $libraryRoot 'examples'
if (!(Test-Path -LiteralPath (Join-Path $examplesRoot 'zolo.toml'))) { throw 'Library examples consumer is missing.' }
Add-Type @'
using System;
using System.Collections.Concurrent;
using System.Diagnostics;
using System.Runtime.InteropServices;
public static class UiDevReceipt {
    public static ConcurrentQueue<string> Lines = new ConcurrentQueue<string>();
    public static void Capture(Process p) {
        p.OutputDataReceived += (_, e) => { if (e.Data != null) Lines.Enqueue(e.Data); };
        p.ErrorDataReceived += (_, e) => { if (e.Data != null) Lines.Enqueue(e.Data); };
        p.BeginOutputReadLine(); p.BeginErrorReadLine();
    }
    [DllImport("user32.dll")] static extern bool PostMessage(IntPtr hwnd, uint msg, UIntPtr w, IntPtr l);
    [DllImport("user32.dll")] static extern uint GetDpiForWindow(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] static extern bool IsWindow(IntPtr hwnd);
    [DllImport("user32.dll")] static extern bool SetForegroundWindow(IntPtr hwnd);
    public static void RestoreForeground(IntPtr hwnd) { if (hwnd != IntPtr.Zero && IsWindow(hwnd)) SetForegroundWindow(hwnd); }
    public static void Click(Process p) {
        p.Refresh(); var hwnd = p.MainWindowHandle;
        if (hwnd == IntPtr.Zero) throw new Exception("Owned child window is absent");
        var dpi = GetDpiForWindow(hwnd); var xy = (int)(40 * (dpi == 0 ? 1 : dpi / 96.0));
        var point = new IntPtr((xy << 16) | xy);
        PostMessage(hwnd, 0x200, UIntPtr.Zero, point);
        PostMessage(hwnd, 0x201, new UIntPtr(1), point);
        PostMessage(hwnd, 0x202, UIntPtr.Zero, point);
    }
}
'@
$previousForeground = [UiDevReceipt]::GetForegroundWindow()
$taskDir = Join-Path $examplesRoot ("target/dev-verification/" + [Guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory($taskDir) | Out-Null
$receiptPath = Join-Path $taskDir 'receipt.log'
$taskEntry = Join-Path $taskDir 'dev-counter.zolo'
$taskModule = Join-Path $taskDir 'dev_component.zolo'
$taskView = Join-Path $taskDir 'dev_view.zolo'
$taskUtf8 = [Text.UTF8Encoding]::new($false)
function Write-View([string]$label, [int]$step = 1, [string]$tail = '') {
    $taskSource = @'
use gpui::{run, state, Node, Column, Text, Button, Style}
use dev_component::label
use dev_view::View
let count = state("counter", 0)
fn Counter() -> Node {
    print("UI_RENDER:LABEL:{count.value}:{label()}")
    <Column style={Style.new(padding: 20.0)}>
        <Button style={Style.new(width: 200.0, height: 60.0)} on_click={|| { count.value += STEP }}>
            <Text value={"LABEL: {count.value}"} />
        </Button>
    </Column>
}
run(Counter, title: "Zolo dev reload verification", width: 400.0, height: 240.0)
TAIL
'@
    [IO.File]::WriteAllText($taskEntry, $taskSource.Replace('LABEL', $label).Replace('STEP', "$step").Replace('TAIL', $tail), $taskUtf8)
}
$taskHistory = [Collections.Generic.List[string]]::new()
function Wait-Receipt([string]$needle, [int]$timeout = 90) {
    $taskTimer = [Diagnostics.Stopwatch]::StartNew()
    while ($taskTimer.Elapsed.TotalSeconds -lt $timeout) {
        $taskLine = ''
        while ([UiDevReceipt]::Lines.TryDequeue([ref]$taskLine)) {
            $taskHistory.Add($taskLine)
            Write-Host $taskLine
            if ($taskLine.Contains('initial run failed')) { throw "Initial UI failed: $taskLine" }
            if ($taskLine.Contains($needle)) { return [math]::Round($taskTimer.Elapsed.TotalMilliseconds, 1) }
        }
        if ($taskProcess.HasExited) { throw "Child exited $($taskProcess.ExitCode) before receipt: $needle" }
        Start-Sleep -Milliseconds 10
    }
    throw "Missing receipt: $needle"
}
function Write-Component([string]$label, [int]$step = 1) {
    $taskSource = @'
use gpui::{state, Node, Column, Text, Button, Style}
struct CounterModel { value: int }
let count = state("component-counter", CounterModel { value: 0 })
pub fn View() -> Node {
    print("UI_COMPONENT:LABEL:{count.value.value}")
    <Column style={Style.new(padding: 20.0)}>
        <Button style={Style.new(width: 200.0, height: 60.0)} on_click={|| { count.update(|model| { model.value += STEP; return model }) }}>
            <Text value={"LABEL: {count.value.value}"} />
        </Button>
    </Column>
}
'@
    [IO.File]::WriteAllText($taskView, $taskSource.Replace('LABEL', $label).Replace('STEP', "$step"), $taskUtf8)
}
[IO.File]::WriteAllText($taskModule, 'pub fn label() -> str { return "module-before" }', $taskUtf8)
Write-Component 'before'
Write-View 'before'
$taskInfo = [Diagnostics.ProcessStartInfo]::new($Zolo)
$taskInfo.WorkingDirectory = $examplesRoot
$taskInfo.UseShellExecute = $false
$taskInfo.CreateNoWindow = $true
$taskInfo.RedirectStandardOutput = $true
$taskInfo.RedirectStandardError = $true
$taskInfo.Environment.Remove('ZOLO_DEV_TIMINGS') | Out-Null
$taskInfo.ArgumentList.Add('dev')
$taskInfo.ArgumentList.Add($taskEntry)
$taskProcess = [Diagnostics.Process]::new()
$taskProcess.StartInfo = $taskInfo
try {
    $taskProcess.Start() | Out-Null
    [UiDevReceipt]::Capture($taskProcess)
    $boot = Wait-Receipt 'UI_RENDER:before:0:module-before' 240
    $taskWindowTimer = [Diagnostics.Stopwatch]::StartNew()
    do {
        if ($taskProcess.HasExited) { throw 'Dev child exited before its native window appeared.' }
        $taskProcess.Refresh()
        $taskWindow = $taskProcess.MainWindowHandle
        if ($taskWindow -ne [IntPtr]::Zero) { break }
        if ($taskWindowTimer.Elapsed.TotalSeconds -gt 20) { throw 'Dev child did not create its native window.' }
        Start-Sleep -Milliseconds 20
    } while ($true)
    # The initial render callback can precede native window creation/painting.
    Start-Sleep -Milliseconds 150
    [UiDevReceipt]::Click($taskProcess)
    Wait-Receipt 'UI_RENDER:before:1:module-before' | Out-Null
    $taskTimer = [Diagnostics.Stopwatch]::StartNew()
    Write-View 'after'
    Wait-Receipt 'UI_RENDER:after:1:module-before' | Out-Null
    $entryMs = [math]::Round($taskTimer.Elapsed.TotalMilliseconds, 1)
    [IO.File]::WriteAllText($taskEntry, 'fn Counter( {', $taskUtf8)
    Wait-Receipt 're-run failed' | Out-Null
    [UiDevReceipt]::Click($taskProcess)
    Wait-Receipt 'UI_RENDER:after:2:module-before' | Out-Null
    Write-View 'rejected' 1 'error("expected reload failure")'
    Wait-Receipt 're-run failed' | Out-Null
    [UiDevReceipt]::Click($taskProcess)
    Wait-Receipt 'UI_RENDER:after:3:module-before' | Out-Null
    $taskTimer.Restart()
    Write-View 'recovered' 2
    Wait-Receipt 'UI_RENDER:recovered:3:module-before' | Out-Null
    $recoveryMs = [math]::Round($taskTimer.Elapsed.TotalMilliseconds, 1)
    [UiDevReceipt]::Click($taskProcess)
    Wait-Receipt 'UI_RENDER:recovered:5:module-before' | Out-Null
    $taskTimer.Restart()
    [IO.File]::WriteAllText($taskModule, 'pub fn label() -> str { return "module-after" }', $taskUtf8)
    Wait-Receipt 'UI_RENDER:recovered:5:module-after' | Out-Null
    $moduleMs = [math]::Round($taskTimer.Elapsed.TotalMilliseconds, 1)
    [IO.File]::WriteAllText($taskModule, "use plugin rust::{rust}`nextern rust from `"missing.rs`" use {missing}`npub fn label() -> str { return `"unadmitted-native-module`" }", $taskUtf8)
    Wait-Receipt 'native session module' | Out-Null
    [UiDevReceipt]::Click($taskProcess)
    Wait-Receipt 'UI_RENDER:recovered:7:module-after' | Out-Null
    [IO.File]::WriteAllText($taskModule, 'pub fn label() -> str { return "module-restored" }', $taskUtf8)
    Wait-Receipt 'UI_RENDER:recovered:7:module-restored' | Out-Null
    [IO.File]::WriteAllText($taskEntry, 'use gpui::run
use dev_view::View
run(View, title: "Zolo dev reload verification", width: 400.0, height: 240.0)', $taskUtf8)
    Wait-Receipt 'UI_COMPONENT:before:0' | Out-Null
    [UiDevReceipt]::Click($taskProcess)
    Wait-Receipt 'UI_COMPONENT:before:1' | Out-Null
    $taskTimer.Restart()
    Write-Component 'after' 3
    Wait-Receipt 'UI_COMPONENT:after:1' | Out-Null
    $componentMs = [math]::Round($taskTimer.Elapsed.TotalMilliseconds, 1)
    [UiDevReceipt]::Click($taskProcess)
    Wait-Receipt 'UI_COMPONENT:after:4' | Out-Null
    $typedSource = [IO.File]::ReadAllText($taskView)
    [IO.File]::WriteAllText($taskView, $typedSource + "`nlet incompatible = state(`"component-counter`", [4])`n", $taskUtf8)
    Wait-Receipt 'changed type; use a new key' | Out-Null
    [UiDevReceipt]::Click($taskProcess)
    Wait-Receipt 'UI_COMPONENT:after:7' | Out-Null
    [IO.File]::WriteAllText($taskView, $typedSource, $taskUtf8)
    Wait-Receipt 'UI_COMPONENT:after:7' | Out-Null
    $propSource = [IO.File]::ReadAllText($taskView).Replace('pub fn View() -> Node {', 'pub fn View(caption: str = "default") -> Node {').Replace('print("UI_COMPONENT:', 'print("UI_PROP:{caption}")' + "`n" + '    print("UI_COMPONENT:')
    [IO.File]::WriteAllText($taskView, $propSource, $taskUtf8)
    Wait-Receipt 'UI_PROP:default' | Out-Null
    [IO.File]::WriteAllText($taskEntry, 'use gpui::run
use dev_view::View
run(|| View(caption: "named"), title: "Zolo dev reload verification", width: 400.0, height: 240.0)', $taskUtf8)
    Wait-Receipt 'UI_PROP:named' | Out-Null
    Wait-Receipt 'UI_COMPONENT:after:7' | Out-Null
    $taskBurst = [IO.File]::ReadAllText($taskEntry)
    $taskTimer.Restart()
    foreach ($caption in @('first', 'second', 'latest')) {
        [IO.File]::WriteAllText($taskEntry, $taskBurst.Replace('"named"', '"' + $caption + '"'), $taskUtf8)
        Start-Sleep -Milliseconds 10
    }
    Wait-Receipt 'UI_PROP:latest' | Out-Null
    Wait-Receipt 'UI_COMPONENT:after:7' | Out-Null
    $burstMs = [math]::Round($taskTimer.Elapsed.TotalMilliseconds, 1)
    [UiDevReceipt]::Click($taskProcess)
    Wait-Receipt 'UI_COMPONENT:after:10' | Out-Null
    $taskProcess.Refresh()
    if ($taskProcess.MainWindowHandle -ne $taskWindow) { throw 'Reload replaced the window' }
    $taskProcess.CloseMainWindow() | Out-Null
    if (!$taskProcess.WaitForExit(15000)) { throw 'Closing the window did not stop the dev command' }
    if ($taskProcess.ExitCode -ne 0) { throw "Dev child exit $($taskProcess.ExitCode)" }
    $taskReceipt = "DEV_RECEIPT boot=$($boot)ms entry=$($entryMs)ms recovery=$($recoveryMs)ms module=$($moduleMs)ms component=$($componentMs)ms burst=$($burstMs)ms measurement=render_callback state=7 component_state=10 nominal_schema_preserved=true incompatible_schema_rejected=true same_window=true close=0 native_session_guard=true new_named_prop=true rapid_saves=true"
    $taskHistory.Add($taskReceipt)
    Write-Host $taskReceipt
} finally {
    [IO.File]::WriteAllLines($receiptPath, $taskHistory, $taskUtf8)
    if (!$taskProcess.HasExited) { $taskProcess.Kill($true); $taskProcess.WaitForExit() }
    $taskProcess.Dispose()
    [UiDevReceipt]::RestoreForeground($previousForeground)
    Write-Host "DEV_GATE receipt: $receiptPath"
}
