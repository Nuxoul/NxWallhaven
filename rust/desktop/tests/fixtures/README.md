# Original preview regression fixture

`preview-12000x8000.jpg` is a synthetic, solid RGB (64, 128, 192) JPEG generated with Pillow 12.1.1:

```python
Image.new('RGB', (12000, 8000), (64, 128, 192)).save(path, quality=75, optimize=True)
```

It contains no user content. The compressed file is about 550 KiB, but the RGB decode needs 288,000,000 bytes and RGBA output needs 384,000,000 bytes. It deterministically reproduced `Memory limit exceeded` at `ImageLoader::preview` before the original-preview limit was removed. The test uses an actual localhost HTTP response and asserts native 12000×8000 output, not a downscaled preview. This test uses several hundred MiB of working memory.
