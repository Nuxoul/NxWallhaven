"""Generate the native ICO from the same geometry/colors as ui/icons/mark.svg."""
from pathlib import Path
from PIL import Image, ImageDraw
p=Path(__file__).with_name('wallhaven-studio.ico')
scale=16
im=Image.new('RGBA',(24*scale,24*scale),(0,0,0,0))
d=ImageDraw.Draw(im)
d.rounded_rectangle((scale,scale,23*scale-1,23*scale-1),radius=6*scale,fill='#24364c')
points=[(5*scale,17*scale),(10*scale,7*scale),(13*scale,13*scale),(15*scale,9*scale),(19*scale,17*scale)]
d.line(points,fill='#79b8ff',width=2*scale,joint='curve')
for x,y in points:d.ellipse((x-scale,y-scale,x+scale,y+scale),fill='#79b8ff')
im.resize((256,256),Image.Resampling.LANCZOS).save(p.with_suffix('.png'))
im.resize((256,256),Image.Resampling.LANCZOS).save(p,sizes=[(16,16),(24,24),(32,32),(48,48),(64,64),(128,128),(256,256)])
print(p)
