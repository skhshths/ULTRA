param([int]$root, [int]$cpuSeconds = 0)
# Every process in the browser's tree: memory as Task Manager shows it (private working set),
# commit (private bytes), and CPU used over a short idle window.
$all = Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId, Name, CreationDate
$born = @{}; foreach ($p in $all) { $born[[int]$p.ProcessId] = $p.CreationDate }
$tree = @{ $root = $true }
do {
  $grew = $false
  foreach ($p in $all) {
    if (-not $tree.ContainsKey([int]$p.ProcessId) -and $tree.ContainsKey([int]$p.ParentProcessId) -and $p.ProcessId -ne $p.ParentProcessId -and
        # Windows reuses process IDs: a real child is never older than its parent.
        $p.CreationDate -ge $born[[int]$p.ParentProcessId]) {
      $tree[[int]$p.ProcessId] = $true; $grew = $true
    }
  }
} while ($grew)
$ids = @($tree.Keys)
$perf = Get-CimInstance Win32_PerfRawData_PerfProc_Process | Where-Object { $ids -contains [int]$_.IDProcess }
$ws = ($perf | Measure-Object -Property WorkingSetPrivate -Sum).Sum
$priv = ($perf | Measure-Object -Property PrivateBytes -Sum).Sum
$names = ($perf | Group-Object Name | ForEach-Object { ($_.Name -replace '#\d+$', '') } | Sort-Object -Unique) -join ','
$cpu = $null
if ($cpuSeconds -gt 0) {
  # Only processes alive at both ends count (one that exits mid-window would make it negative).
  $a = @{}; Get-Process -Id $ids -ErrorAction SilentlyContinue | ForEach-Object { $a[$_.Id] = $_.TotalProcessorTime.TotalMilliseconds }
  Start-Sleep -Seconds $cpuSeconds
  $used = 0
  Get-Process -Id $ids -ErrorAction SilentlyContinue | ForEach-Object { if ($a.ContainsKey($_.Id)) { $used += $_.TotalProcessorTime.TotalMilliseconds - $a[$_.Id] } }
  $cpu = [math]::Round($used / ($cpuSeconds * 1000) * 100 / [Environment]::ProcessorCount, 2)
}
@{ procs = $perf.Count; wsMB = [math]::Round($ws / 1MB, 1); privMB = [math]::Round($priv / 1MB, 1); cpuPct = $cpu; names = $names } | ConvertTo-Json -Compress
