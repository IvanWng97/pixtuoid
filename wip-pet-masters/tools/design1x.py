import sys, pathlib
P="/private/tmp/claude-501/-Users-navepnow-Desktop-pixtuoid-nosync/c3372352-26a1-4356-bb82-c52383d069de/scratchpad/critters"
sys.path.insert(0,P)
from preview1x import sheet
sys.argv=["x","/tmp/none","cat_walk",P+"/blender/cat8","125"]
exec(open(P+"/blender/install_walk.py").read().split("frames = grounded")[0])
BOB={"cat":[0,0,1,0,0,0,1,0],"dog":[0,1,2,1,0,1,2,1]}
# the leg row at 1x, apart for two frames then together for two, as a 2-beat walk reads
LEGS={"cat":["..t...t.","..t...t.","...t.t..","...t.t..","..t...t.","..t...t.","...t.t..","...t.t.."],
      "dog":[".x...x..",".x...x..","..x.x...","..x.x...",".x...x..",".x...x..","..x.x...","..x.x..."]}
EXTRA={"cat":[(4,0,"u"),(6,0,"u"),(1,1,"u")],"dog":[(5,0,"x"),(4,1,"z")]}
def design(sp):
    d=P+f"/blender/{sp}8"
    fs=grounded([rows(p) for p in sorted(pathlib.Path(d).glob(f"{sp}_walk_*.sprite"), key=lambda p:int(p.stem.rsplit("_",1)[1]))])
    out=[]
    for i,(g,b) in enumerate(zip(fs,BOB[sp])):
        r=read([["."]*len(g[0])]*b+g[:len(g)-b])
        base=[row[:] for row in r]
        for x,y,k in EXTRA[sp]: r[y][x]=k
        for x,k in enumerate(LEGS[sp][i]): r[5][x]=k
        changed=sum((base[y][x]==".")!=(r[y][x]==".") for y in range(6) for x in range(8))
        out.append((f"{sp} {i} ({changed})", r))
    return out
named=design("cat")+design("dog")
for n,g in named:
    print(n); print("\n".join("".join(r) for r in g))
sheet(named, P+"/walk1x_design.png")
for sp in ("cat","dog"):
    od=pathlib.Path(P+f"/blender/{sp}8_1x"); od.mkdir(exist_ok=True)
    for i,(n,g) in enumerate(design(sp)):
        (od/f"{sp}_walk_{i}.sprite").write_text("@frame 0\n"+"\n".join(" ".join(r) for r in g)+"\n",encoding="utf-8")
