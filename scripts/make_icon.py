"""Generates the app icon (src-tauri/icons/source.png). Run: python3 scripts/make_icon.py"""
from PIL import Image, ImageDraw

S = 1024
img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
# vertical gradient background
bg = Image.new("RGBA", (S, S))
top, bottom = (36, 104, 96), (17, 52, 58)
for y in range(S):
    t = y / (S - 1)
    c = tuple(int(top[i] + (bottom[i] - top[i]) * t) for i in range(3)) + (255,)
    ImageDraw.Draw(bg).line([(0, y), (S, y)], fill=c)
mask = Image.new("L", (S, S), 0)
ImageDraw.Draw(mask).rounded_rectangle([40, 40, S - 40, S - 40], radius=220, fill=255)
img.paste(bg, (0, 0), mask)
d = ImageDraw.Draw(img)
heights = [0.26, 0.48, 0.70, 0.42, 0.58, 0.30]
w, gap = 74, 46
total = len(heights) * w + (len(heights) - 1) * gap
x = (S - total) // 2
for h in heights:
    hh = int(S * h * 0.78)
    y0 = (S - hh) // 2
    d.rounded_rectangle([x, y0, x + w, y0 + hh], radius=w // 2, fill=(250, 248, 242, 255))
    x += w + gap
img.save("src-tauri/icons/source.png")
print("ok")
