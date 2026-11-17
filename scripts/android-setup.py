#!/usr/bin/env python3
"""Run once after `cargo tauri android init`. Adds what the generated Android project lacks for Sleeve:
permissions to read (and, where the system allows, write) music on shared storage, and a prompt for them on first launch."""
import pathlib, re, sys

root = pathlib.Path(__file__).resolve().parent.parent / "src-tauri/gen/android/app/src/main"
manifest = root / "AndroidManifest.xml"
if not manifest.exists():
    sys.exit("Run `cargo tauri android init` first: " + str(manifest) + " not found")

PERMS = [
    ('android.permission.READ_MEDIA_AUDIO', ''),
    ('android.permission.READ_MEDIA_IMAGES', ''),                                  # cover.jpg / folder.jpg beside the music
    ('android.permission.READ_EXTERNAL_STORAGE', ' android:maxSdkVersion="32"'),
    ('android.permission.WRITE_EXTERNAL_STORAGE', ' android:maxSdkVersion="29"'),   # tag edits on Android 10 and older
    ('android.permission.VIBRATE', ''),
    ('android.permission.INTERNET', ''),                                           # listen-together link
]
xml = manifest.read_text()
add = "".join(f'    <uses-permission android:name="{n}"{extra} />\n' for n, extra in PERMS if n not in xml)
if add:
    xml = re.sub(r"(<manifest[^>]*>\n?)", lambda m: m.group(1) + add, xml, count=1)
    manifest.write_text(xml)
# DSD is streamed to the app's own player from a server bound to this device (127.0.0.1), over plain HTTP.
if "usesCleartextTraffic" in xml:
    xml = re.sub(r'android:usesCleartextTraffic="[^"]*"', 'android:usesCleartextTraffic="true"', xml)
else:
    xml = xml.replace("<application", '<application android:usesCleartextTraffic="true"', 1)
manifest.write_text(xml)
print("manifest: added", add.count("uses-permission"), "permissions; local HTTP allowed")

# Ask for the media permissions when the app starts, so a scan can see the Music folder.
ASK = """
        if (android.os.Build.VERSION.SDK_INT >= 33) {
            requestPermissions(arrayOf(android.Manifest.permission.READ_MEDIA_AUDIO, android.Manifest.permission.READ_MEDIA_IMAGES), 7)
        } else {
            requestPermissions(arrayOf(android.Manifest.permission.READ_EXTERNAL_STORAGE, android.Manifest.permission.WRITE_EXTERNAL_STORAGE), 7)
        }
"""
for kt in root.rglob("MainActivity.kt"):
    src = kt.read_text()
    if "READ_MEDIA_AUDIO" in src:
        print("activity: already patched")
    elif "super.onCreate(savedInstanceState)" in src:
        kt.write_text(src.replace("super.onCreate(savedInstanceState)", "super.onCreate(savedInstanceState)" + ASK, 1))
        print("activity: permission prompt added to onCreate")
    else:
        body = "\n    override fun onCreate(savedInstanceState: android.os.Bundle?) {\n        super.onCreate(savedInstanceState)" + ASK + "    }\n"
        i = src.rstrip().rfind("}")
        kt.write_text(src[:i] + body + "}\n")
        print("activity: onCreate with permission prompt added")
