Get-ChildItem -Directory | ForEach-Object {
    $s = (Get-ChildItem $_.FullName -Recurse -File -ErrorAction SilentlyContinue | Measure-Object -Property Length -Sum).Sum
    '{0,-14} {1,8:N1} MB' -f $_.Name, ($s / 1MB)
}
$t = (Get-ChildItem -Recurse -File -ErrorAction SilentlyContinue | Measure-Object -Property Length -Sum).Sum
'{0,-14} {1,8:N1} MB' -f 'TOTAL', ($t / 1MB)
