import base64, json, sys, urllib.parse
def varint(b,i):
    r=s=0
    while True:
        c=b[i]; i+=1; r|=(c&0x7f)<<s; s+=7
        if c<0x80: return r,i
def parse(b, depth=0):
    i=0; out=[]
    while i<len(b):
        k,i=varint(b,i); f,w=k>>3,k&7
        if w==0: v,i=varint(b,i); out.append((f,v))
        elif w==2:
            n,i=varint(b,i); sub=b[i:i+n]; i+=n
            try:
                s=sub.decode(); 
                if s.isprintable(): out.append((f,s)); continue
            except: pass
            out.append((f,parse(sub,depth+1)))
        else: raise ValueError(w)
    return out
def enc_varint(v):
    o=bytearray()
    while True:
        b=v&0x7f; v>>=7
        if v: o.append(b|0x80)
        else: o.append(b); return bytes(o)
def encode(msg):
    o=bytearray()
    for f,v in msg:
        if isinstance(v,int): o+=enc_varint(f<<3)+enc_varint(v)
        else:
            sub=v.encode() if isinstance(v,str) else encode(v)
            o+=enc_varint(f<<3|2)+enc_varint(len(sub))+sub
    return bytes(o)
def b64d(s): s=s.replace('-','+').replace('_','/'); return base64.b64decode(s+'='*(-len(s)%4))
def b64e(b): return base64.urlsafe_b64encode(b).decode().rstrip('=')
if __name__=="__main__":
    url=json.load(open(sys.argv[1]))['url']
    tfs=urllib.parse.parse_qs(urllib.parse.urlparse(url).query)['tfs'][0]
    m=parse(b64d(tfs)); print(json.dumps(m))
    assert b64e(encode(m))==tfs.rstrip('='), "roundtrip mismatch"
    print("roundtrip ok")
