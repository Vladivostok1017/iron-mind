import os, sys, subprocess, glob

FF = r"F:\python313\Lib\site-packages\imageio_ffmpeg\binaries\ffmpeg-win-x86_64-v7.1.exe"
SRC = r"C:\Users\Administrator\AppData\Roaming\dsh-desktop\harness\attachments\v1\files\15\15604fc19b4cf0e3168f9886f9094cdc3883fd42d53f0c99d8a923667bae5ed0\dl (51) (10).mp4"
OUTDIR = r"F:\dsh\iron-mind\docs\_video\ocr_frames"
STEP = 20  # seconds per frame

os.makedirs(OUTDIR, exist_ok=True)

existing = glob.glob(os.path.join(OUTDIR, "o_*.png"))
if len(existing) < 100:
    print("extracting frames every %ds ..." % STEP, flush=True)
    subprocess.run([
        FF, "-hide_banner", "-loglevel", "error",
        "-i", SRC,
        "-vf", "fps=1/%d,scale=1400:-2" % STEP,
        "-start_number", "0",
        "-y", os.path.join(OUTDIR, "o_%03d.png"),
    ], check=True)

frames = sorted(glob.glob(os.path.join(OUTDIR, "o_*.png")))
print("frames:", len(frames), flush=True)

from cnocr import CnOcr
ocr = CnOcr()

out_path = r"F:\dsh\iron-mind\docs\_video\ocr_text.txt"
with open(out_path, "w", encoding="utf-8") as fh:
    for i, f in enumerate(frames):
        ts = i * STEP
        mm, ss = divmod(ts, 60)
        try:
            res = ocr.ocr(f)
            texts = [r["text"].strip() for r in res if r["text"].strip()]
        except Exception as e:
            texts = ["<OCR_ERROR %s>" % e]
        header = "=== [%02d:%02d] frame %03d ===" % (mm, ss, i)
        fh.write(header + "\n")
        for t in texts:
            fh.write(t + "\n")
        fh.write("\n")
        fh.flush()
        if i % 10 == 0:
            print("done", i, "/", len(frames), flush=True)

print("ALL DONE", out_path, flush=True)
