Dev helpers (agent sender-net owns fake-sender.py and udp-listen.py):
- `fake-sender.py` — emits synthetic protocol-v1 packets (several fake hosts, random walk values) by UDP broadcast and optionally TCP, for viewer UI work without a real daemon.
- `udp-listen.py` — prints every packet heard on UDP 48000, for verifying the sender on the LAN.
