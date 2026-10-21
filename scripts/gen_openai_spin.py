"""Generate a 4-frame 32px spritesheet of the OpenAI logo spinning.

The logo has 6-fold symmetry, so 4 frames covering 0..60 degrees loop seamlessly.
"""
import math, sys
from PIL import Image, ImageDraw

S = 32          # sprite size
SS = 32         # supersample factor
FRAMES = 4
W = S * SS


def petal_mask(angle_deg):
    """Six tilted rounded-rect loops arranged around the center."""
    m = Image.new("L", (W, W), 0)
    for k in range(6):
        layer = Image.new("L", (W, W), 0)
        d = ImageDraw.Draw(layer)
        cx, cy = W / 2, W / 2 - 8.6 * SS
        hw, hh = 3.4 * SS, 6.4 * SS
        d.rounded_rectangle([cx - hw, cy - hh, cx + hw, cy + hh],
                            radius=3.4 * SS, outline=255, width=int(1.7 * SS))
        layer = layer.rotate(-60, center=(cx, cy), resample=Image.BICUBIC)
        layer = layer.rotate(-(angle_deg + k * 60), center=(W / 2, W / 2),
                             resample=Image.BICUBIC)
        m = Image.composite(Image.new("L", (W, W), 255), m, layer)
    return m


def frame(i):
    m = petal_mask(i * 15)
    m = m.resize((S, S), Image.BOX).point(lambda v: 255 if v >= 110 else 0)
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    img.paste((255, 255, 255, 255), mask=m)
    return img


sheet = Image.new("RGBA", (S * FRAMES, S), (0, 0, 0, 0))
for i in range(FRAMES):
    sheet.paste(frame(i), (S * i, 0))
sheet.save(sys.argv[1])
bg = Image.new("RGBA", sheet.size, (40, 40, 48, 255))
bg.alpha_composite(sheet)
bg.resize((S * FRAMES * 8, S * 8), Image.NEAREST).save(sys.argv[2])
