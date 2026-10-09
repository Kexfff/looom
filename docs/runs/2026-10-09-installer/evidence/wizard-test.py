import os,pty,select,time,re,hashlib,termios
from pathlib import Path
assert Path('/sys/class/net/enp1s0/address').read_text().strip()=='52:54:00:7b:23:63'
binary='/var/lib/looom/dev/installer-20261009/looom'
base=Path('/var/lib/looom/dev/installer-20261009')
def digest():
    with open('/dev/vdb','rb',buffering=0) as f:
        first=f.read(1048576); f.seek(-1048576,2);last=f.read()
    return hashlib.sha256(first+last).hexdigest()
before=digest()
for mode in ['cancel','root-mismatch','user-mismatch']:
    pid,fd=pty.fork()
    if pid==0: os.execv(binary,[binary,'install'])
    output=b''; cursor=0
    def expect(pattern,answer,hidden=False):
        global output,cursor
        deadline=time.monotonic()+90
        while True:
            match=re.search(pattern,output[cursor:])
            if match:
                cursor+=match.end()
                if hidden: assert not (termios.tcgetattr(fd)[3]&termios.ECHO),'secret input echo enabled'
                os.write(fd,(answer() if callable(answer) else answer)+b'\n'); return
            assert time.monotonic()<deadline,'prompt timeout'
            if select.select([fd],[],[],1)[0]:output+=os.read(fd,65536)
    for pattern,answer in [(b'Disk to erase.*: ',b'/dev/vdb'),(b'Computer name.*: ',b''),(b'User name.*: ',b''),(b'Timezone.*: ',b''),(b'Locale.*: ',b''),(b'Console keymap.*: ',b''),(b'SSH public key file.*: ',b'')]:expect(pattern,answer)
    if mode=='cancel': expect(b'Type the exact confirmation.*: ',b'CANCEL')
    else:
        expect(b'Type the exact confirmation.*: ',lambda:re.search(rb'ERASE /dev/vdb [^\r\n]+',output).group(0))
        secret=os.urandom(24).hex().encode()
        expect(b'root password: ',secret,True)
        expect(b'Repeat password: ',secret+b'wrong' if mode=='root-mismatch' else secret,True)
        if mode=='user-mismatch':
            expect(b'user password: ',secret,True)
            expect(b'Repeat password: ',secret+b'wrong',True)
    while True:
        try:
            data=os.read(fd,65536)
            if not data:break
            output+=data
        except OSError:break
    _,status=os.waitpid(pid,0); os.close(fd)
    assert status!=0,'unexpected installer success'
    assert (b'installation cancelled' if mode=='cancel' else b'passwords differ') in output,'unexpected failure'
    if mode!='cancel':assert secret not in output,'secret echoed'
    assert digest()==before,'disk bytes changed'
    print('PASS: terminal wizard '+mode+'; disk unchanged'+('; passwords hidden' if mode!='cancel' else ''))
