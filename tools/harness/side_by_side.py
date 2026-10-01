"""Render our rooftop once and save real-vs-ours side by side to referen/harness/rooftop_compare.png."""
import os, sys
from PIL import Image, ImageDraw
sys.path.insert(0, os.path.dirname(__file__))
from shot import render, ROOT

ours = os.path.join(ROOT, "target", "harness", "ours_rooftop.png")
render("rooftop", ours, {})
real = Image.open(os.path.join(ROOT, "referen", "harness", "live", "roof_1.png")).convert("RGB").resize((960, 540))
mine = Image.open(ours).convert("RGB").resize((960, 540))
out = Image.new("RGB", (1920, 580), (20, 20, 20))
out.paste(real, (0, 40)); out.paste(mine, (960, 40))
d = ImageDraw.Draw(out)
d.text((10, 12), "REAL GAME (your capture)", fill=(255, 255, 255))
d.text((970, 12), "OURS (current build)", fill=(255, 255, 255))
import time; path = os.path.join(ROOT, "referen", "harness", f"rooftop_compare_{time.strftime('%H%M%S')}.png")
out.save(path)
print(path)
