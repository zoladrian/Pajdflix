"""Exercise the actual Windows probe against isolated, simulated OS responses.

No VPN/firewall changes, network connections, or application compilation.
"""
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[1]
source = (ROOT / "app/src/vpn.rs").read_text(encoding="utf-8")
probe = re.search(r'const ROUTE_PROBE: &str = r#"(.*?)"#;', source, re.S)[1]

def fixture(adapter_index=67, route_index=67, ipv6=False, fail=False):
    adapter = "" if adapter_index is None else f"[pscustomobject]@{{Name='PrivadoVPN_Wireguard';Status='Up';InterfaceIndex={adapter_index}}}"
    route = "throw 'probe unavailable'" if fail else f"[pscustomobject]@{{CimClass=[pscustomobject]@{{CimClassName='MSFT_NetRoute'}};InterfaceIndex={route_index}}}"
    ip = "[pscustomobject]@{IPAddress='2001:db8::1';InterfaceIndex=4}" if ipv6 else ""
    return f"function Get-NetAdapter {{ {adapter} }}\nfunction Find-NetRoute {{ {route} }}\nfunction Get-NetIPAddress {{ {ip} }}\n"

cases = [
    ("IPv4 routed through Privado, no outside IPv6", fixture(), 0),
    ("Privado application running but tunnel absent", fixture(adapter_index=None), 2),
    ("connected adapter with traffic still routed through Wi-Fi", fixture(route_index=4), 2),
    ("IPv4 protected but IPv6 available outside VPN", fixture(ipv6=True), 4),
    ("OS route query failed", fixture(fail=True), 3),
]
for name, setup, expected in cases:
    # Match the application's -Command invocation; -File depends on the
    # machine's script execution policy and can fail before running a probe.
    result = subprocess.run(["powershell.exe", "-NoProfile", "-NonInteractive", "-Command", setup + probe], capture_output=True, timeout=12)
    assert result.returncode == expected, f"{name}: expected {expected}, got {result.returncode}; stderr={result.stderr!r}"
    print("PASS:", name)
