#!/usr/bin/env python3
import os
import socket

def address(name):
    host, port = os.environ[name].rsplit(":", 1)
    return host, int(port)

listen, upstream = address("REDIRECT_LISTEN"), address("REDIRECT_UPSTREAM")
sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
sock.bind(listen)
while True:
    payload, _peer = sock.recvfrom(65535)
    sock.sendto(payload, upstream)
