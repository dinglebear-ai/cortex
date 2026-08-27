#!/usr/bin/env python3
import http.client, http.server, ssl, sys
upstream_host, upstream_port, listen_port, cert, key = sys.argv[1:]
class Proxy(http.server.BaseHTTPRequestHandler):
    def proxy(self):
        body=self.rfile.read(int(self.headers.get('content-length','0')))
        c=http.client.HTTPConnection(upstream_host,int(upstream_port),timeout=15)
        headers={k:v for k,v in self.headers.items() if k.lower() not in {'host','connection','content-length'}}
        headers['Host']='localhost'; headers['Content-Length']=str(len(body))
        c.request(self.command,self.path,body,headers); r=c.getresponse(); data=r.read()
        self.send_response(r.status)
        for k,v in r.getheaders():
            if k.lower() not in {'connection','transfer-encoding','content-length'}: self.send_header(k,v)
        self.send_header('Content-Length',str(len(data))); self.end_headers(); self.wfile.write(data)
    do_GET=proxy; do_POST=proxy
    def log_message(self,*args): pass
s=http.server.ThreadingHTTPServer(('127.0.0.1',int(listen_port)),Proxy)
ctx=ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER); ctx.load_cert_chain(cert,key); s.socket=ctx.wrap_socket(s.socket,server_side=True); s.serve_forever()
