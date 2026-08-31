#!/usr/bin/env python3
import argparse,json,socket,time
p=argparse.ArgumentParser(); p.add_argument('--host',default='127.0.0.1'); p.add_argument('--port',type=int,required=True); p.add_argument('--prefix',required=True); p.add_argument('--count',type=int,required=True); p.add_argument('--delay',type=float,default=.003); a=p.parse_args()
assert 1<=a.count<=10000
sent=[]; failed=[]
for i in range(a.count):
 marker=f'{a.prefix}-{i:05d}'
 msg=f'<14>1 {time.strftime("%Y-%m-%dT%H:%M:%S.000Z",time.gmtime())} concurrency-live cortex - - - {marker}\n'.encode()
 try:
  with socket.create_connection((a.host,a.port),timeout=2) as s:s.sendall(msg)
  sent.append(marker)
 except OSError:failed.append(marker)
 time.sleep(a.delay)
print(json.dumps({'schema':'cortex-live-producer-v1','offered':a.count,'accepted':len(sent),'rejected':len(failed),'sent':sent,'failed':failed},separators=(',',':')))
