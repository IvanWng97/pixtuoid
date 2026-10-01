"""Render the cat's walk motion as flat material-ID maps, one per frame.

Run: blender -b --factory-startup --python cat_walk_rig.py -- OUT_DIR [FRAMES]

Only the motion comes from here: the legs (thigh + shin + paw per leg, diagonal
pairs half a cycle apart, a lifted swing) and the tail (a chain lagging the
stride), plus the body's bob. Units are master pixels: x right, y DOWN in the
32x24 frame, the frame faces east. Each part renders in a flat ID colour at 4x
oversample with no anti-aliasing; `compose.py` reads the IDs back per pixel and
lays the locked torso over them.
"""
import math
import sys

import bpy

argv = sys.argv[sys.argv.index("--") + 1:]
OUT = argv[0]
FRAMES = int(argv[1]) if len(argv) > 1 else 8
W, H, OVER = 32, 24, 4

# ID colours, one per part; compose.py keys them back
IDS = {
    "leg_near": (1.0, 0.0, 0.0),
    "leg_far": (0.0, 1.0, 0.0),
    "paw_near": (1.0, 1.0, 0.0),
    "paw_far": (0.0, 1.0, 1.0),
    "tail": (0.0, 0.0, 1.0),
    "tail_tip": (1.0, 0.0, 1.0),
}

# the rig, in master pixels (east-facing frame, y down)
BELLY_Y = 16.5          # where a leg meets the body
GROUND_Y = 21.5         # the paw's row
THIGH, SHIN = 2.6, 2.6  # segment lengths
LEG_R = 1.0             # 2 px wide
HIPS = {"near_hind": 10.5, "far_hind": 12.5, "far_fore": 21.5, "near_fore": 23.5}
STRIDE = 2.5            # how far a paw travels each side of its hip
LIFT = 1.6              # a swinging paw's height at mid-swing
STANCE = 0.55           # the share of a cycle a paw is down
# each leg's phase: a diagonal pair together, the other half a cycle off
PHASE = {"near_fore": 0.0, "far_hind": 0.0, "far_fore": 0.5, "near_hind": 0.5}
BOB = 0.6               # the body's rise at each passing
TAIL_BASE = (10.0, 10.0)
TAIL_SEGS = 9
TAIL_SEG_LEN = 1.05
TAIL_R = 1.0


def foot(t, hip_x):
    """Where a paw is at cycle phase t in [0, 1): down and sliding back for
    STANCE, then lifted and swung forward."""
    if t < STANCE:
        u = t / STANCE
        return hip_x + STRIDE * (1 - 2 * u), GROUND_Y
    u = (t - STANCE) / (1 - STANCE)
    x = hip_x - STRIDE + 2 * STRIDE * (0.5 - 0.5 * math.cos(math.pi * u))
    return x, GROUND_Y - LIFT * math.sin(math.pi * u)


def knee(hip, paw, forward):
    """Two-bone IK in the side plane: the knee bends forward for a fore leg,
    back for a hind leg."""
    hx, hy = hip
    px, py = paw
    d = min(math.hypot(px - hx, py - hy), THIGH + SHIN - 1e-3)
    a = math.atan2(py - hy, px - hx)
    cos_k = (THIGH ** 2 + d ** 2 - SHIN ** 2) / (2 * THIGH * d)
    k = math.acos(max(-1.0, min(1.0, cos_k)))
    a += -k if forward else k
    return hx + THIGH * math.cos(a), hy + THIGH * math.sin(a)


def to_world(x, y, depth):
    # frame x right, y down -> Blender x right, z up; depth along +y (away)
    return (x, depth, H - y)


def material(name):
    m = bpy.data.materials.new(name)
    m.diffuse_color = (*IDS[name], 1.0)
    return m


def capsule(a, b, r, mat, depth):
    """A cylinder from frame point a to b with round ends, in `mat`."""
    (ax, ay), (bx, by) = a, b
    A, B = to_world(ax, ay, depth), to_world(bx, by, depth)
    length = math.dist(A, B)
    mid = tuple((p + q) / 2 for p, q in zip(A, B))
    bpy.ops.mesh.primitive_cylinder_add(radius=r, depth=max(length, 1e-3), location=mid, vertices=16)
    cyl = bpy.context.object
    dx, dz = B[0] - A[0], B[2] - A[2]
    cyl.rotation_euler = (0.0, math.atan2(dx, dz), 0.0)
    cyl.data.materials.append(mat)
    for p in (A, B):
        bpy.ops.mesh.primitive_uv_sphere_add(radius=r, location=p, segments=16, ring_count=8)
        bpy.context.object.data.materials.append(mat)


def clear():
    for o in list(bpy.data.objects):
        if o.type == "MESH":
            bpy.data.objects.remove(o, do_unlink=True)


def setup():
    bpy.ops.wm.read_factory_settings(use_empty=True)
    scene = bpy.context.scene
    scene.render.engine = "BLENDER_WORKBENCH"
    scene.display.shading.light = "FLAT"
    scene.display.shading.color_type = "MATERIAL"
    scene.display.render_aa = "OFF"
    scene.view_settings.view_transform = "Standard"
    scene.render.resolution_x = W * OVER
    scene.render.resolution_y = H * OVER
    scene.render.film_transparent = True
    scene.render.image_settings.file_format = "PNG"
    scene.render.image_settings.color_mode = "RGBA"
    cam_data = bpy.data.cameras.new("cam")
    cam_data.type = "ORTHO"
    cam_data.ortho_scale = W
    cam = bpy.data.objects.new("cam", cam_data)
    scene.collection.objects.link(cam)
    cam.location = (W / 2, -50.0, H / 2)
    cam.rotation_euler = (math.pi / 2, 0.0, 0.0)
    scene.camera = cam
    return scene


def frame(i, mats):
    clear()
    t0 = i / FRAMES
    bob = -BOB * (0.5 - 0.5 * math.cos(4 * math.pi * t0))  # up at each passing
    for leg, hip_x in HIPS.items():
        near = leg.startswith("near")
        fore = leg.endswith("fore")
        depth = -2.0 if near else 2.0
        t = (t0 + PHASE[leg]) % 1.0
        hip = (hip_x, BELLY_Y + bob)
        paw = foot(t, hip_x)
        k = knee(hip, paw, forward=fore)
        capsule(hip, k, LEG_R, mats["leg_near" if near else "leg_far"], depth)
        capsule(k, (paw[0], paw[1] - 0.6), LEG_R, mats["leg_near" if near else "leg_far"], depth)
        capsule((paw[0] - 0.2, paw[1]), (paw[0] + 1.0, paw[1]), 0.55, mats["paw_near" if near else "paw_far"], depth - 0.5)
    # the tail: up and back from the rump, each segment lagging the one before
    x, y = TAIL_BASE[0], TAIL_BASE[1] + bob
    ang = math.radians(-108)  # up and back (frame y is down)
    pts = [(x, y)]
    for s in range(TAIL_SEGS):
        sway = math.radians(14) * math.sin(2 * math.pi * t0 - 0.55 * s)
        curl = math.radians(38) * max(0, s - 5)  # the tip hooks back over
        a = ang + sway + curl
        x += TAIL_SEG_LEN * math.cos(a)
        y += TAIL_SEG_LEN * math.sin(a)
        pts.append((x, y))
    for s in range(TAIL_SEGS):
        name = "tail_tip" if s >= TAIL_SEGS - 2 else "tail"
        capsule(pts[s], pts[s + 1], TAIL_R, mats[name], 1.0)
    bpy.context.scene.render.filepath = f"{OUT}/id_{i:02d}.png"
    bpy.ops.render.render(write_still=True)
    with open(f"{OUT}/bob_{i:02d}.txt", "w") as f:
        f.write(f"{bob}\n")


scene = setup()
mats = {n: material(n) for n in IDS}
for i in range(FRAMES):
    frame(i, mats)
print("rendered", FRAMES)
