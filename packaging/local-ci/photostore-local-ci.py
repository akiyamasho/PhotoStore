#!/usr/bin/env python3
"""Opt-in PhotoStore macOS CI/release watcher."""
import datetime, fcntl, json, os, plistlib, re, shutil, subprocess, sys, tempfile, time
from pathlib import Path

HOME = Path(os.environ.get("HOME", str(Path.home()))).expanduser()
for p in (HOME / ".cargo/bin", Path("/opt/homebrew/bin"), Path("/usr/local/bin"), Path("/usr/bin"), Path("/bin")):
    if str(p) not in os.environ.get("PATH", "").split(":"): os.environ["PATH"] = f"{p}:{os.environ.get('PATH','')}"
ROOT = Path(os.environ.get("PHOTOSTORE_LOCAL_CI_ROOT", HOME / "Library/Application Support/PhotoStore/local-ci")); STATE=ROOT/"state"; CHECKOUTS=ROOT/"checkouts"; STAGE=ROOT/"staged"
LOG=ROOT/"local-ci.log"; LOCK=ROOT/"poll.lock"; TARGET=Path("/Users/computer/Desktop/dev/PhotoStore/target")
DEV_WATCH_LEASE=ROOT/"dev-watch.lease"
PROD=Path("/Applications/PhotoStore.app"); DEV=Path("/Applications/PhotoStore-dev.app"); REPO="akiyamasho/PhotoStore"; DEV_KEYS=Path("/Users/computer/Desktop/dev/tools/Animation/photostore_keys.dev.json")
SHA_RE=re.compile(r"^[0-9a-fA-F]{40}$"); TAG_RE=re.compile(r"^[A-Za-z0-9][A-Za-z0-9._/-]{0,127}$")

def tool(name):
    p=os.environ.get("PHOTOSTORE_LOCAL_CI_"+name.upper()) or shutil.which(name)
    if not p: raise RuntimeError("required executable unavailable: "+name)
    return p
def atomic(path,data):
    path.parent.mkdir(parents=True,exist_ok=True); fd,tmp=tempfile.mkstemp(prefix="."+path.name+".",dir=path.parent)
    try:
        with os.fdopen(fd,"wb") as f: f.write(data); f.flush(); os.fsync(f.fileno())
        os.replace(tmp,path); d=os.open(path.parent,os.O_RDONLY); os.fsync(d); os.close(d)
    finally:
        if os.path.exists(tmp): os.unlink(tmp)
def atext(p,s): atomic(p,s.encode())
def ajson(p,v): atext(p,json.dumps(v,indent=2,sort_keys=True)+"\n")
def txt(p): return p.read_text().strip() if p.exists() else ""
def jread(p,default=None): return json.loads(p.read_text()) if p.exists() else default
def log(s):
    ROOT.mkdir(parents=True,exist_ok=True)
    with LOG.open("a") as f: f.write(datetime.datetime.now(datetime.timezone.utc).isoformat()+" "+s+"\n")
def run(argv,cwd=None,env=None,capture=False,timeout=90):
    e=os.environ.copy(); e.update(env or {}); return subprocess.run(argv,cwd=cwd,env=e,check=True,text=True,capture_output=capture,timeout=timeout)
def gh(path):
    try:
        a=[tool("gh"),"api",path]+(["--paginate","--slurp"] if "releases?" in path else []); v=json.loads(run(a,capture=True,timeout=60).stdout)
        return [x for page in v for x in page] if "releases?" in path else v
    except Exception as e: raise RuntimeError("GitHub metadata unavailable: "+str(e))
def sha(v):
    v=str(v).strip();
    if not SHA_RE.fullmatch(v): raise RuntimeError("invalid SHA")
    return v.lower()
def release_sha(tag):
    if not TAG_RE.fullmatch(tag) or ".." in tag or tag.startswith("-"): raise RuntimeError("invalid release tag")
    o=gh(f"repos/{REPO}/git/ref/tags/{tag}").get("object",{})
    for _ in range(4):
        s=sha(o.get("sha","")); typ=o.get("type")
        if typ=="commit": return s
        if typ!="tag": raise RuntimeError("tag does not resolve to commit")
        o=gh(f"repos/{REPO}/git/tags/{s}").get("object",{})
    raise RuntimeError("tag indirection too deep")
def latest_release():
    r=[x for x in gh(f"repos/{REPO}/releases?per_page=100") if not x.get("draft") and not x.get("prerelease") and x.get("published_at")]
    return max(r,key=lambda x:x["published_at"]) if r else None
def main_sha(): return sha(gh(f"repos/{REPO}/branches/main")["commit"]["sha"])
def checkout(name,s):
    p=CHECKOUTS/name; CHECKOUTS.mkdir(parents=True,exist_ok=True)
    if not (p/".git").exists(): run([tool("git"),"clone","--no-checkout",f"https://github.com/{REPO}.git",str(p)],timeout=90)
    run([tool("git"),"-C",str(p),"fetch","--no-tags","origin",s],timeout=90); run([tool("git"),"-C",str(p),"checkout","--detach","--force",s],timeout=90); return p
def procs(): return subprocess.run(["/bin/ps","-axo","pid=,command="],text=True,capture_output=True,check=True).stdout.splitlines()
def running():
    p=re.compile(r"^/Applications/PhotoStore(?:-[0-9][^/]*)?\.app/Contents/MacOS/PhotoStore(?:\s|$)"); return any(len(x.split(None,1))==2 and p.match(x.split(None,1)[1]) for x in procs())
def running_dev(): return any("/Applications/PhotoStore-dev.app/Contents/MacOS/PhotoStore-dev" in x for x in procs())
def dev_watch_active():
    d=jread(DEV_WATCH_LEASE,{}) or {}; pid=d.get("watcher_pid"); uid=d.get("uid")
    if not isinstance(pid,int) or pid<=0 or uid!=os.getuid(): return False
    try: os.kill(pid,0)
    except OSError: return False
    return any(line.split(None,2)[0]==str(pid) and "photostore-dev.py" in line for line in procs() if len(line.split(None,2))==3)
def provenance(binary,s,dev=False):
    t=run([str(binary),"--version"],capture=True,timeout=15).stdout
    if s[:8].lower() not in t.lower(): raise RuntimeError("binary provenance mismatch")
    if dev and "PhotoStore-dev build" not in t: raise RuntimeError("dev marker missing")
    if not dev and "PhotoStore-dev build" in t: raise RuntimeError("development binary rejected for production")
def font_manifest():
    f=os.environ.get("CRAFT_FONTS_DIR",""); p=Path(f)
    if not f or not (p/"fonts").is_dir() or not (p/"fonts/manifest.txt").is_file() or not (p/"fonts/manifest.txt").read_text().strip(): raise RuntimeError("CRAFT_FONTS_DIR/fonts/manifest.txt is required")
    return p
def licenses(dest,source,production):
    dest.mkdir(parents=True,exist_ok=True)
    for n in ("LICENSE","LICENSE-MIT","LICENSE-APACHE","NOTICE","ATTRIBUTION.md","COPYRIGHT"):
        if (source/n).exists(): shutil.copy2(source/n,dest/n)
    asset_root=source/"assets"
    if asset_root.is_dir():
        for p in asset_root.rglob("*"):
            if p.is_file() and (p.name.startswith("LICENSE") or p.name.startswith("NOTICE")):
                target=dest/"Assets"/p.relative_to(asset_root); target.parent.mkdir(parents=True,exist_ok=True); shutil.copy2(p,target)
    if os.environ.get("CRAFT_FONTS_DIR"):
        for p in font_manifest().glob("fonts/*/OFL.txt"): shutil.copy2(p,dest/("OFL-"+p.parent.name+".txt"))
def bundle(source,binary,s,dev):
    out=STAGE/(f"{'dev' if dev else 'release'}-{s}.app"); shutil.rmtree(out,ignore_errors=True); (out/"Contents/MacOS").mkdir(parents=True); (out/"Contents/Resources").mkdir()
    exe="PhotoStore-dev" if dev else "PhotoStore"; shutil.copy2(binary,out/"Contents/MacOS"/exe); shutil.copy2(source/"assets/app-icon/photostore.icns",out/"Contents/Resources/PhotoStore.icns")
    v=next(x.split('"')[1] for x in (source/"Cargo.toml").read_text().splitlines() if x.startswith("version = ")); pl=plistlib.loads((source/"packaging/macos/Info.plist.in").read_text().replace("@VERSION@",v).replace("@SHORT_VERSION@",v.split("-")[0]).replace("@BUILD_SHA@",s).encode())
    if dev:
        for k in ("CFBundleDocumentTypes","UTExportedTypeDeclarations","UTImportedTypeDeclarations"): pl.pop(k,None)
        pl.update(CFBundleDisplayName="PhotoStore-dev",CFBundleName="PhotoStore-dev",CFBundleExecutable="PhotoStore-dev",CFBundleIdentifier="ai.photostore.app.dev")
    atomic(out/"Contents/Info.plist",plistlib.dumps(pl,fmt=plistlib.FMT_XML)); licenses(out/"Contents/Resources/Licenses",source,not dev); run([tool("codesign"),"--force","--deep","--sign","-",str(out)],timeout=90); run([tool("codesign"),"--verify","--deep","--strict",str(out)],timeout=90); provenance(out/"Contents/MacOS"/exe,s,dev); return out
def install(src,dst,guard):
    if guard(): return False
    staged=dst.parent/("."+dst.stem+f".staged.{os.getpid()}.app"); old=dst.parent/("."+dst.stem+f".backup.{int(time.time())}.app"); shutil.rmtree(staged,ignore_errors=True); shutil.copytree(src,staged); run([tool("codesign"),"--verify","--deep","--strict",str(staged)],timeout=90)
    if guard(): shutil.rmtree(staged); return False
    if dst.exists(): os.replace(dst,old)
    try: os.replace(staged,dst)
    except Exception:
        if old.exists() and not dst.exists(): os.replace(old,dst)
        raise
    return True
def seed_dev_preferences():
    d=HOME/"Library/Application Support/PhotoStore-dev/preferences.json"
    if d.exists(): return
    source=HOME/"Library/Application Support/PhotoStore/preferences.json"; data={}
    if source.exists():
        data=json.loads(source.read_text())
        if not isinstance(data,dict): raise RuntimeError("production preferences must be a JSON object")
    data.update({"shortcutFile":str(DEV_KEYS),"disableRecording":True}); ajson(d,data)
def build_main(s):
    p=checkout("main",s); font_manifest(); e={"PHOTOSTORE_BUILD_SHA":s,"PHOTOSTORE_BUILD_DATE":datetime.date.today().isoformat(),"CARGO_TARGET_DIR":str(TARGET),"CARGO_BUILD_JOBS":"1","RAYON_NUM_THREADS":"1","CRAFT_FONTS_REQUIRED":"1","CRAFT_FONTS_DIR":os.environ.get("CRAFT_FONTS_DIR","")}; run(["nice","-n","10",tool("cargo"),"build","--locked","--offline","-p","photostore","--bin","photostore","--features","dev-install,heif"],cwd=p,env=e,timeout=None)
    f=STAGE/f"photostore-dev-{s}"; shutil.copy2(TARGET/"debug/photostore",f); provenance(f,s,True); a=bundle(p,f,s,True); pending={"sha":s,"app":str(a)}
    ajson(STATE/"dev.pending.json",pending)
    if running_dev() or dev_watch_active(): return
    if install(a,DEV,running_dev): seed_dev_preferences(); atext(STATE/"main.sha",s); (STATE/"dev.pending.json").unlink(missing_ok=True); (STATE/"main.failed").unlink(missing_ok=True)
def build_release(rel,s):
    p=checkout("release",s); font_manifest(); e={"PHOTOSTORE_BUILD_SHA":s,"PHOTOSTORE_BUILD_DATE":datetime.date.today().isoformat(),"CARGO_TARGET_DIR":str(TARGET),"CARGO_BUILD_JOBS":"1","RAYON_NUM_THREADS":"1","CRAFT_FONTS_REQUIRED":"1","CRAFT_FONTS_DIR":os.environ["CRAFT_FONTS_DIR"]}; run(["nice","-n","10",tool("cargo"),"build","--release","--locked","-p","photostore","--features","heif"],cwd=p,env=e,timeout=None)
    f=STAGE/f"photostore-release-{s}"; shutil.copy2(TARGET/"release/photostore",f); provenance(f,s,False); a=bundle(p,f,s,False); ajson(STATE/"release.pending.json",{"id":rel["id"],"tag":rel["tag_name"],"sha":s,"app":str(a)})
    if not running() and install(a,PROD,running): ajson(STATE/"release.json",jread(STATE/"release.pending.json")); (STATE/"release.pending.json").unlink(missing_ok=True); (STATE/"release.failed").unlink(missing_ok=True)
def discard(p,d):
    a=Path(d.get("app",""));
    if a.is_relative_to(STAGE) and a.exists(): shutil.rmtree(a)
    p.unlink(missing_ok=True)
def finish_pending(m,rel):
    p=STATE/"dev.pending.json"; d=jread(p)
    if d and d.get("sha")!=m: discard(p,d); d=None
    if d and not dev_watch_active() and Path(d.get("app","")).exists() and install(Path(d["app"]),DEV,running_dev): seed_dev_preferences(); atext(STATE/"main.sha",d["sha"]); p.unlink(missing_ok=True)
    p=STATE/"release.pending.json"; d=jread(p); ident=((rel or {}).get("id"),(rel or {}).get("tag"),(rel or {}).get("sha"))
    if d and (d.get("id"),d.get("tag"),d.get("sha"))!=ident: discard(p,d); d=None
    if d and not running() and Path(d.get("app","")).exists() and install(Path(d["app"]),PROD,running): ajson(STATE/"release.json",d); p.unlink(missing_ok=True)
def bootstrap():
    if (STATE/"bootstrapped").exists(): log("already bootstrapped; preserving state"); return
    m=main_sha(); r=latest_release(); a=None
    if r: a={"id":r["id"],"tag":r["tag_name"],"sha":release_sha(r["tag_name"])}
    atext(STATE/"main.observed.sha",m)
    if a: ajson(STATE/"release.observed.json",a)
    atext(STATE/"bootstrapped",datetime.datetime.now(datetime.timezone.utc).isoformat()+"\n"); log("baseline recorded; no build performed")
def poll(observe=False):
    if not (STATE/"bootstrapped").exists(): log("not bootstrapped; skipping"); return
    m=main_sha(); r=latest_release(); rel=None
    if r: rel={"id":r["id"],"tag":r["tag_name"],"sha":release_sha(r["tag_name"])}
    if observe: log(f"observe-only metadata main={m} release={(r or {}).get('tag_name','')}"); return
    previous=jread(STATE/"release.json",{}) or jread(STATE/"release.observed.json",{}) or {}; pending_release=jread(STATE/"release.pending.json",{}) or {}
    drift = rel and ((previous.get("id") == rel.get("id") and previous.get("sha") and previous.get("sha") != rel.get("sha")) or (pending_release.get("id") == rel.get("id") and pending_release.get("sha") and pending_release.get("sha") != rel.get("sha")))
    if drift:
        reason={"reason":"release tag drift","id":rel["id"],"tag":rel["tag"],"old_sha":previous.get("sha") or pending_release.get("sha"),"new_sha":rel["sha"]}; log(json.dumps(reason,sort_keys=True)); ajson(STATE/"release.failed",reason); rel=None
    finish_pending(m,rel); old=txt(STATE/"main.sha") or txt(STATE/"main.observed.sha"); pd=jread(STATE/"dev.pending.json",{}) or {}; fail=STATE/"main.failed"
    if m!=old and pd.get("sha")!=m and ((not fail.exists()) or txt(fail)!=m or time.time()-fail.stat().st_mtime>=300):
        fail.unlink(missing_ok=True)
        try: build_main(m)
        except Exception as e: log(f"main build failed sha={m}: {e}"); atext(fail,m)
    old=jread(STATE/"release.json",{}) or jread(STATE/"release.observed.json",{}) or {}; pd=jread(STATE/"release.pending.json",{}) or {}; fail=STATE/"release.failed"; ident=((rel or {}).get("id"),(rel or {}).get("tag"),(rel or {}).get("sha"))
    if rel and ident!=(old.get("id"),old.get("tag"),old.get("sha")) and not pd and (not fail.exists() or time.time()-fail.stat().st_mtime>=300):
        fail.unlink(missing_ok=True)
        try: build_release(r,rel["sha"])
        except Exception as e: log(f"release build failed tag={rel['tag']}: {e}"); atext(fail,"\t".join(map(str,ident)))
def status(): print(json.dumps({"root":str(ROOT),"bootstrapped":(STATE/"bootstrapped").exists(),"main_installed":txt(STATE/"main.sha") or None,"main_observed":txt(STATE/"main.observed.sha") or None,"release_installed":jread(STATE/"release.json"),"release_observed":jread(STATE/"release.observed.json"),"dev_pending":(STATE/"dev.pending.json").exists(),"release_pending":(STATE/"release.pending.json").exists(),"main_failed":(STATE/"main.failed").exists(),"release_failed":(STATE/"release.failed").exists()},indent=2))
def main():
    a=sys.argv[1:]
    if a==["--status"]: status(); return
    ROOT.mkdir(parents=True,exist_ok=True); STATE.mkdir(exist_ok=True); STAGE.mkdir(exist_ok=True)
    with LOCK.open("a") as f:
        try: fcntl.flock(f,fcntl.LOCK_EX|fcntl.LOCK_NB)
        except BlockingIOError: log("another poll is active; skipping"); return
        if a==["--bootstrap"]: bootstrap()
        elif a==["--observe-only"]: poll(True)
        elif not a: poll()
        else: raise SystemExit("usage: photostore-local-ci.py [--bootstrap|--observe-only|--status]")
if __name__=="__main__": main()
