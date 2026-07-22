#!/usr/bin/env python3
"""Send SSDP M-SEARCH out EACH local IPv4 interface (multi-NIC machines send
multicast on only one by default). Prints which interface, if any, the WiiM /
MediaRenderers answer on."""
import socket
import time
import ssl
import re
import urllib.request

MSEARCH = (
    "M-SEARCH * HTTP/1.1\r\n"
    "HOST: 239.255.255.250:1900\r\n"
    'MAN: "ssdp:discover"\r\n'
    "MX: 2\r\n"
    "ST: ssdp:all\r\n\r\n"
).encode()

def local_ipv4s():
    ips = set()
    for info in socket.getaddrinfo(socket.gethostname(), None, socket.AF_INET):
        ips.add(info[4][0])
    # also the default-route source IP
    try:
        s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        s.connect(("8.8.8.8", 80))
        ips.add(s.getsockname()[0])
        s.close()
    except Exception:
        pass
    return sorted(ips)

def probe_iface(ip):
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM, socket.IPPROTO_UDP)
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    try:
        s.bind((ip, 0))
    except OSError as e:
        return f"bind failed: {e}", []
    s.setsockopt(socket.IPPROTO_IP, socket.IP_MULTICAST_IF, socket.inet_aton(ip))
    s.settimeout(0.4)
    for _ in range(2):
        try:
            s.sendto(MSEARCH, ("239.255.255.250", 1900))
        except OSError as e:
            return f"send failed: {e}", []
    found = {}
    end = time.time() + 3
    while time.time() < end:
        try:
            data, addr = s.recvfrom(4096)
            txt = data.decode(errors="ignore")
            loc = re.search(r"(?im)^location:\s*(.+)$", txt)
            st = re.search(r"(?im)^(st|nt):\s*(.+)$", txt)
            found.setdefault(addr[0], (loc.group(1).strip() if loc else "", st.group(2).strip() if st else ""))
        except socket.timeout:
            pass
    s.close()
    return "ok", sorted(found.items())

print("Local IPv4 interfaces:", local_ipv4s())
for ip in local_ipv4s():
    status, devices = probe_iface(ip)
    print(f"\n=== iface {ip}: {status}, {len(devices)} responders")
    for host, (loc, st) in devices:
        tag = " <-- MediaRenderer" if "MediaRenderer" in st else ""
        print(f"   {host}  {st[:60]}{tag}")
        if "MediaRenderer" in st and loc:
            print(f"        {loc}")
