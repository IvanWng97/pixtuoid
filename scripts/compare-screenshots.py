#!/usr/bin/env python3
import sys
from pathlib import Path
from PIL import Image, ImageChops

def compare_images(ref_path, cand_path, diff_path, threshold_percent=0.01):
    ref_path = Path(ref_path)
    cand_path = Path(cand_path)
    diff_path = Path(diff_path)

    if not ref_path.exists():
        print(f"ERROR: Reference image does not exist at {ref_path}")
        return False
    if not cand_path.exists():
        print(f"ERROR: Candidate image does not exist at {cand_path}")
        return False

    ref = Image.open(ref_path).convert("RGBA")
    cand = Image.open(cand_path).convert("RGBA")

    if ref.size != cand.size:
        print(f"ERROR: Image sizes differ. Reference: {ref.size}, Candidate: {cand.size}")
        return False

    diff = ImageChops.difference(ref, cand)
    bbox = diff.getbbox()
    if bbox is None:
        print("SUCCESS: Images are visually identical.")
        return True

    # Generate a visual difference overlay highlighting mismatches in red.
    diff_data = diff.load()
    ref_data = ref.load()
    cand_data = cand.load()

    w, h = ref.size
    mismatched = 0

    out = Image.new("RGBA", (w, h))
    out_data = out.load()

    for y in range(h):
        for x in range(w):
            r_diff = diff_data[x, y]
            # Check for any diff in the RGB channels
            if r_diff[0] > 0 or r_diff[1] > 0 or r_diff[2] > 0:
                mismatched += 1
                out_data[x, y] = (255, 0, 0, 255)  # highlight mismatched pixels in red
            else:
                # Dim the candidate image background pixels to make the red overlay pop
                c_pix = cand_data[x, y]
                out_data[x, y] = (c_pix[0] // 2, c_pix[1] // 2, c_pix[2] // 2, 255)

    total_pixels = w * h
    percent = (mismatched / total_pixels) * 100.0
    print(f"MISMATCH: {mismatched} / {total_pixels} pixels differ ({percent:.4f}%)")

    diff_path.parent.mkdir(parents=True, exist_ok=True)
    out.save(diff_path)
    print(f"Difference highlight image saved to: {diff_path}")

    # Allow a tiny threshold for float coordinate differences if any exist.
    if percent > threshold_percent:
        print(f"ERROR: Mismatch is {percent:.4f}% which is greater than the allowed threshold of {threshold_percent}%.")
        return False

    print("SUCCESS: Mismatch is within the acceptable threshold.")
    return True

if __name__ == "__main__":
    if len(sys.argv) < 4:
        print("Usage: compare-screenshots.py <reference_path> <candidate_path> <diff_out_path>")
        sys.exit(2)

    success = compare_images(sys.argv[1], sys.argv[2], sys.argv[3])
    sys.exit(0 if success else 1)
