from PIL import Image, ImageDraw, ImageFilter
import math
from pathlib import Path


SIZE = 1024
SCALE = 3
CANVAS = SIZE * SCALE
OUT = Path("src-tauri/icons/protohub-icon-source.png")


def lerp(a, b, t):
    return int(a + (b - a) * t)


def mix(c1, c2, t):
    return tuple(lerp(c1[i], c2[i], t) for i in range(4))


def rounded_mask(size, radius):
    mask = Image.new("L", (size, size), 0)
    draw = ImageDraw.Draw(mask)
    draw.rounded_rectangle((0, 0, size - 1, size - 1), radius=radius, fill=255)
    return mask


def line(draw, points, fill, width):
    draw.line([(int(x), int(y)) for x, y in points], fill=fill, width=width, joint="curve")


def circle(draw, x, y, r, fill, outline=None, width=1):
    box = (int(x - r), int(y - r), int(x + r), int(y + r))
    draw.ellipse(box, fill=fill, outline=outline, width=width)


def rounded_rect(draw, box, radius, fill, outline=None, width=1):
    draw.rounded_rectangle(tuple(int(v) for v in box), radius=int(radius), fill=fill, outline=outline, width=int(width))


img = Image.new("RGBA", (CANVAS, CANVAS), (0, 0, 0, 0))

# Full icon body with a subtle diagonal gradient.
body = Image.new("RGBA", (CANVAS, CANVAS), (0, 0, 0, 0))
px = body.load()
top = (15, 33, 45, 255)
mid = (24, 61, 75, 255)
bottom = (53, 56, 119, 255)
for y in range(CANVAS):
    for x in range(CANVAS):
        t = (x * 0.38 + y * 0.88) / (CANVAS * 1.26)
        if t < 0.55:
            color = mix(top, mid, t / 0.55)
        else:
            color = mix(mid, bottom, (t - 0.55) / 0.45)
        px[x, y] = color

mask = rounded_mask(CANVAS, 232 * SCALE)
img.alpha_composite(Image.composite(body, Image.new("RGBA", (CANVAS, CANVAS), (0, 0, 0, 0)), mask))

# Soft glow behind the protocol mark.
glow = Image.new("RGBA", (CANVAS, CANVAS), (0, 0, 0, 0))
g = ImageDraw.Draw(glow)
circle(g, 512 * SCALE, 520 * SCALE, 330 * SCALE, (72, 214, 190, 62))
circle(g, 650 * SCALE, 360 * SCALE, 210 * SCALE, (129, 105, 255, 72))
glow = glow.filter(ImageFilter.GaussianBlur(62 * SCALE))
img.alpha_composite(glow)

draw = ImageDraw.Draw(img)

# Contained chip shape: recognizable at small sizes without the old white square.
panel_shadow = Image.new("RGBA", (CANVAS, CANVAS), (0, 0, 0, 0))
ps = ImageDraw.Draw(panel_shadow)
rounded_rect(ps, (235 * SCALE, 251 * SCALE, 789 * SCALE, 805 * SCALE), 128 * SCALE, (0, 0, 0, 92))
panel_shadow = panel_shadow.filter(ImageFilter.GaussianBlur(24 * SCALE))
img.alpha_composite(panel_shadow)

panel_box = (236 * SCALE, 236 * SCALE, 788 * SCALE, 788 * SCALE)
rounded_rect(draw, panel_box, 128 * SCALE, (11, 27, 37, 250), (111, 236, 216, 255), 20 * SCALE)
rounded_rect(draw, (280 * SCALE, 280 * SCALE, 744 * SCALE, 744 * SCALE), 94 * SCALE, (14, 35, 47, 255), (246, 255, 252, 88), 8 * SCALE)

# Protocol routes.
nodes = {
    "left_top": (395 * SCALE, 390 * SCALE),
    "left_bottom": (395 * SCALE, 635 * SCALE),
    "center": (512 * SCALE, 512 * SCALE),
    "right_top": (636 * SCALE, 390 * SCALE),
    "right_bottom": (636 * SCALE, 635 * SCALE),
}

line(draw, [nodes["left_top"], nodes["center"], nodes["right_top"]], (112, 236, 216, 255), 24 * SCALE)
line(draw, [nodes["left_bottom"], nodes["center"], nodes["right_bottom"]], (139, 116, 255, 255), 24 * SCALE)
line(draw, [nodes["left_top"], nodes["right_bottom"]], (247, 255, 252, 76), 12 * SCALE)
line(draw, [nodes["left_bottom"], nodes["right_top"]], (247, 255, 252, 76), 12 * SCALE)

# Light code-bracket hints connect the icon to proto files without adding text.
line(draw, [(340 * SCALE, 470 * SCALE), (294 * SCALE, 512 * SCALE), (340 * SCALE, 554 * SCALE)], (247, 255, 252, 210), 22 * SCALE)
line(draw, [(684 * SCALE, 470 * SCALE), (730 * SCALE, 512 * SCALE), (684 * SCALE, 554 * SCALE)], (247, 255, 252, 210), 22 * SCALE)

# Central hub ring.
circle(draw, *nodes["center"], 72 * SCALE, (12, 30, 42, 255), (247, 255, 252, 255), 16 * SCALE)
circle(draw, *nodes["center"], 34 * SCALE, (118, 236, 216, 255))

for name, (x, y) in nodes.items():
    if name == "center":
        continue
    circle(draw, x, y, 46 * SCALE, (12, 30, 42, 255), (247, 255, 252, 255), 12 * SCALE)
    fill = (128, 105, 255, 255) if "top" in name else (111, 232, 214, 255)
    circle(draw, x, y, 24 * SCALE, fill)

# A few restrained highlights for polish.
highlight = Image.new("RGBA", (CANVAS, CANVAS), (0, 0, 0, 0))
h = ImageDraw.Draw(highlight)
rounded_rect(h, (116 * SCALE, 88 * SCALE, 908 * SCALE, 512 * SCALE), 190 * SCALE, (255, 255, 255, 22))
highlight = highlight.filter(ImageFilter.GaussianBlur(8 * SCALE))
img.alpha_composite(Image.composite(highlight, Image.new("RGBA", (CANVAS, CANVAS), (0, 0, 0, 0)), mask))

img = img.resize((SIZE, SIZE), Image.Resampling.LANCZOS)
OUT.parent.mkdir(parents=True, exist_ok=True)
img.save(OUT)
print(OUT)
