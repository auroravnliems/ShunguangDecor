"""Adds/edits keys in the 10 language files, preserving order, writing LF, UTF-8 without BOM.

Usage: python tools/add-keys.py keys.json
keys.json: {"after": "<existing key, optional>", "keys": {"key": {"vi": "...", "en": "...", ...}}}
Any missing language falls back to the English text (a later translation pass will replace it).
"""
import collections, json, sys, pathlib

LANGS = ["vi", "en", "ru", "zh-CN", "ko", "ja", "de", "es", "pt-BR", "fr"]
root = pathlib.Path(__file__).resolve().parent.parent / "src" / "locales"
spec = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
after = spec.get("after")
keys = spec["keys"]
for code in LANGS:
    p = root / f"{code}.json"
    d = json.loads(p.read_text(encoding="utf-8"), object_pairs_hook=collections.OrderedDict)
    vals = {k: v.get(code, v["en"]) for k, v in keys.items()}
    out = collections.OrderedDict()
    inserted = False
    for k, v in d.items():
        if k in vals:
            continue
        out[k] = v
        if k == after:
            out.update(vals)
            inserted = True
    if not inserted:
        out.update(vals)
    with open(p, "w", encoding="utf-8", newline="") as f:
        f.write(json.dumps(out, ensure_ascii=False, indent=2) + "\n")
print(f"{len(keys)} keys x {len(LANGS)} languages")
