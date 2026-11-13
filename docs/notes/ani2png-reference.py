"""Convert Windows .ani/.cur cursor packs to PNG/APNG previews (mockup helper only).

usage: python -I ani2png.py <raw_root> <out_dir>
Each pack folder under raw_root must contain an install.inf somewhere.
"""
import io, json, os, re, struct, sys
from PIL import Image

ROLES = ["Arrow", "Help", "AppStarting", "Wait", "Crosshair", "IBeam", "NWPen", "No",
         "SizeNS", "SizeWE", "SizeNWSE", "SizeNESW", "SizeAll", "UpArrow", "Hand", "Pin", "Person"]


def best_entry(data):
    """Return (PIL image, hotspot) for the largest image in an ICO/CUR blob."""
    _, typ, count = struct.unpack_from("<HHH", data, 0)
    best = None
    for i in range(count):
        w, h, colors, _r, a, b, size, off = struct.unpack_from("<BBBBHHII", data, 6 + 16 * i)
        w = w or 256
        h = h or 256
        if best is None or w * h > best[0] * best[1]:
            best = (w, h, colors, a, b, size, off)
    w, h, colors, a, b, size, off = best
    blob = data[off:off + size]
    hot = (a, b) if typ == 2 else (0, 0)
    if blob[:8] == b"\x89PNG\r\n\x1a\n":
        img = Image.open(io.BytesIO(blob))
    else:
        bits = struct.unpack_from("<H", blob, 14)[0]  # biBitCount
        hdr = struct.pack("<HHH", 0, 1, 1) + struct.pack("<BBBBHHII", w % 256, h % 256, colors, 0, 1, bits, size, 22)
        img = Image.open(io.BytesIO(hdr + blob))
    img.load()
    return img.convert("RGBA"), hot


def read_ani(path):
    raw = open(path, "rb").read()
    if raw[:4] != b"RIFF":  # actually a plain .cur
        img, hot = best_entry(raw)
        return [img], [100], hot
    assert raw[8:12] == b"ACON", path
    pos, end = 12, len(raw)
    frames, rate, seq, jif = [], None, None, 6
    while pos + 8 <= end:
        cid, csz = raw[pos:pos + 4], struct.unpack_from("<I", raw, pos + 4)[0]
        body = raw[pos + 8:pos + 8 + csz]
        if cid == b"anih":
            jif = struct.unpack_from("<9I", body)[7] or 6
        elif cid == b"rate":
            rate = list(struct.unpack_from("<%dI" % (csz // 4), body))
        elif cid == b"seq ":
            seq = list(struct.unpack_from("<%dI" % (csz // 4), body))
        elif cid == b"LIST" and body[:4] == b"fram":
            p = 4
            while p + 8 <= len(body):
                sid, ssz = body[p:p + 4], struct.unpack_from("<I", body, p + 4)[0]
                if sid == b"icon":
                    frames.append(best_entry(body[p + 8:p + 8 + ssz]))
                p += 8 + ssz + (ssz & 1)
        pos += 8 + csz + (csz & 1)
    order = seq if seq else list(range(len(frames)))
    rates = rate if rate else [jif] * len(order)
    imgs = [frames[i][0] for i in order]
    durs = [max(16, int(r * 1000 / 60)) for r in rates]
    return imgs, durs, frames[0][1]


def parse_inf(path):
    txt = open(path, "rb").read().decode("utf-8", "replace").replace("\r", "")
    strings = {}
    for m in re.finditer(r'^\s*(\w+)\s*=\s*"([^"]*)"', txt.split("[Strings]")[-1], re.M):
        strings[m.group(1).lower()] = m.group(2)
    roles = {}
    for m in re.finditer(r'HKCU,"Control Panel\\Cursors",(\w+),0x\w+,"[^"]*%(\w+)%"', txt):
        role, var = m.group(1), m.group(2).lower()
        if var in strings:
            roles[role.lower()] = strings[var]
    return strings.get("scheme_name", os.path.basename(os.path.dirname(path))), roles


def main(root, out):
    os.makedirs(out, exist_ok=True)
    index = []
    for pack in sorted(os.listdir(root)):
        infs = [os.path.join(dp, f) for dp, _, fs in os.walk(os.path.join(root, pack)) for f in fs if f.lower() == "install.inf"]
        if not infs:
            continue
        inf = infs[0]
        name, roles = parse_inf(inf)
        base = os.path.dirname(inf)
        slug = re.sub(r"[^a-z0-9]+", "-", pack.lower()).strip("-")
        entry = {"pack": pack, "scheme": name, "slug": slug, "cursors": {}}
        for role in ROLES:
            fn = roles.get(role.lower())
            if not fn or not os.path.exists(os.path.join(base, fn)):
                continue
            imgs, durs, hot = read_ani(os.path.join(base, fn))
            dst = f"{slug}-{role.lower()}.png"
            if len(imgs) > 1:
                imgs[0].save(os.path.join(out, dst), save_all=True, append_images=imgs[1:], duration=durs,
                             loop=0, disposal=1, blend=0)
            else:
                imgs[0].save(os.path.join(out, dst))
            entry["cursors"][role] = {"file": dst, "frames": len(imgs), "size": imgs[0].size, "hot": hot}
        index.append(entry)
        print(pack, name, len(entry["cursors"]), "roles,", {k: v["frames"] for k, v in list(entry["cursors"].items())[:4]})
    json.dump(index, open(os.path.join(out, "index.json"), "w", encoding="utf-8"), ensure_ascii=False, indent=1)


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
