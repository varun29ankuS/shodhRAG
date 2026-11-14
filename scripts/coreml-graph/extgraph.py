"""plain_fp16.onnx -> a graph whose O4-derived weights are external data read in place from model_O4.onnx.
Usage: extgraph.py <model_O4.onnx> <plain_fp16.onnx> <out.onnx>"""
import sys, onnx
from onnx import TensorProto

o4_path, plain_path, dst = sys.argv[1:4]
data = open(o4_path, "rb").read()


def varint(buf, i):
    shift = result = 0
    while True:
        b = buf[i]; i += 1
        result |= (b & 0x7F) << shift
        if b < 0x80:
            return result, i
        shift += 7


def fields(buf, start, end):
    i = start
    while i < end:
        key, i = varint(buf, i)
        num, wire = key >> 3, key & 7
        if wire == 0:
            _, i = varint(buf, i); continue
        if wire == 1:
            i += 8; continue
        if wire == 5:
            i += 4; continue
        if wire != 2:
            raise ValueError(f"wire type {wire}")
        n, i = varint(buf, i)
        yield num, i, i + n
        i += n


def raw_data_spans():
    spans = {}
    graph = next((s, e) for num, s, e in fields(data, 0, len(data)) if num == 7)
    for num, s, e in fields(data, *graph):
        if num != 5:
            continue
        name = raw = None
        for f, fs, fe in fields(data, s, e):
            if f == 8:
                name = data[fs:fe].decode()
            elif f == 9:
                raw = (fs, fe - fs)
        if name and raw:
            spans[name] = raw
    return spans


spans = raw_data_spans()
m = onnx.load(plain_path)
moved = 0
for t in m.graph.initializer:
    if t.name not in spans or t.data_type != TensorProto.FLOAT16:
        continue
    offset, length = spans[t.name]
    assert data[offset:offset + length] == t.raw_data, t.name
    t.ClearField("raw_data")
    t.data_location = TensorProto.EXTERNAL
    del t.external_data[:]
    for k, v in (("location", "model_O4.onnx"), ("offset", str(offset)), ("length", str(length))):
        e = t.external_data.add(); e.key, e.value = k, v
    moved += 1
onnx.save(m, dst)
print({"moved": moved, "kept_inline": len(m.graph.initializer) - moved, "unaligned": sum(o % 4096 != 0 for o, _ in spans.values())})
