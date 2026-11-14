"""Check native Polish message coverage and formatting placeholders, offline."""
import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
POLISH_LETTERS = set("ąćęłńóśźżĄĆĘŁŃÓŚŹŻ")
UNCHANGED_NAMES = {"Anime", "Fantasy", "Horror", "Musical", "Sport", "Thriller", "Western", "HDR", "AMD FSR", "Usenet"}
MESSAGE = re.compile(r'@tr\(\s*("(?:\\.|[^"\\])*")(?:\s*=>\s*("(?:\\.|[^"\\])*"))?')

def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"Duplicate translation: {key}")
        result[key] = value
    return result

def read(path):
    return json.loads(path.read_text(encoding="utf-8-sig"), object_pairs_hook=unique_object)

def normalize(text):
    return text.replace("%s", "{}").replace("%d", "{}")

def main():
    english = read(ROOT / "vendor/stremio-translations/en-US.json")
    polish = read(ROOT / "vendor/stremio-translations/pl-PL.json")
    native = read(ROOT / "app/i18n/pl-PL.json")
    aliases = {normalize(value): polish[key] for key, value in english.items() if key in polish}
    errors = []
    for source, translation in native.items():
        if not translation.strip() or normalize(source).count("{}") != normalize(translation).count("{}"):
            errors.append(f"Invalid translation or placeholders: {source}")
    checked = 0
    for path in (ROOT / "app/ui").rglob("*.slint"):
        for match in MESSAGE.finditer(path.read_text(encoding="utf-8-sig")):
            key = json.loads(match[1])
            source = normalize(json.loads(match[2] or match[1]))
            checked += 1
            translated = native.get(source, polish.get(key) if match[2] else aliases.get(source))
            if source in native or any(letter in POLISH_LETTERS for letter in source) or source in UNCHANGED_NAMES:
                continue
            if re.search(r"[A-Za-z]{2}", source) and (not translated or translated == source):
                errors.append(f"{path.relative_to(ROOT)}: untranslated: {source}")
    if errors:
        print("\n".join(sorted(set(errors))))
        return 1
    print(f"PASS: {checked} UI messages checked; {len(native)} native Polish translations; no missing translations.")
    return 0

if __name__ == "__main__":
    sys.stdout.reconfigure(encoding="utf-8")
    raise SystemExit(main())
