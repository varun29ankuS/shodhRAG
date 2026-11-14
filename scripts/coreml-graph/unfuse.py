"""model_O4.onnx -> the same fp16 weights in standard ONNX operators (opset 17). Usage: unfuse.py <O4> <out.onnx>"""
import sys, onnx, numpy as np
from onnx import helper as h, numpy_helper as nh, TensorProto as T

src, dst = sys.argv[1:3]
m = onnx.load(src)
g = m.graph
heads, hidden = 12, 768
hd = hidden // heads
extra_inits, out_nodes = [], []

def const(name, arr):
    extra_inits.append(nh.from_array(np.asarray(arr), name)); return name

const("u_axes12", np.array([1, 2], np.int64))
const("u_split", np.array([hidden] * 3, np.int64))
const("u_heads_shape", np.array([0, 0, heads, hd], np.int64))
const("u_merge_shape", np.array([0, 0, hidden], np.int64))
const("u_one", np.array(1, np.float16))
const("u_neg", np.array(-10000, np.float16))
const("u_scale", np.array(1 / np.sqrt(hd), np.float16))
const("u_half", np.array(0.5, np.float16))
const("u_rsqrt2", np.array(1 / np.sqrt(2), np.float16))
out_nodes += [
    h.make_node("Cast", ["attention_mask"], ["u_mask16"], to=T.FLOAT16),
    h.make_node("Sub", ["u_one", "u_mask16"], ["u_unmask"]),
    h.make_node("Mul", ["u_unmask", "u_neg"], ["u_mask_add2"]),
    h.make_node("Unsqueeze", ["u_mask_add2", "u_axes12"], ["u_mask_add"]),
]

def attention(n, i):
    x, w, b = n.input[:3]
    p = f"u_att{i}_"
    q, k, v = p + "q", p + "k", p + "v"
    return [
        h.make_node("MatMul", [x, w], [p + "mm"]),
        h.make_node("Add", [p + "mm", b], [p + "qkv"]),
        h.make_node("Split", [p + "qkv", "u_split"], [q, k, v], axis=-1),
        h.make_node("Mul", [q, "u_scale"], [p + "qs"]),
        h.make_node("Reshape", [p + "qs", "u_heads_shape"], [p + "qr"]),
        h.make_node("Transpose", [p + "qr"], [p + "qt"], perm=[0, 2, 1, 3]),
        h.make_node("Reshape", [k, "u_heads_shape"], [p + "kr"]),
        h.make_node("Transpose", [p + "kr"], [p + "kt"], perm=[0, 2, 3, 1]),
        h.make_node("Reshape", [v, "u_heads_shape"], [p + "vr"]),
        h.make_node("Transpose", [p + "vr"], [p + "vt"], perm=[0, 2, 1, 3]),
        h.make_node("MatMul", [p + "qt", p + "kt"], [p + "s"]),
        h.make_node("Add", [p + "s", "u_mask_add"], [p + "sm"]),
        h.make_node("Softmax", [p + "sm"], [p + "pr"], axis=-1),
        h.make_node("MatMul", [p + "pr", p + "vt"], [p + "c"]),
        h.make_node("Transpose", [p + "c"], [p + "ct"], perm=[0, 2, 1, 3]),
        h.make_node("Reshape", [p + "ct", "u_merge_shape"], [n.output[0]]),
    ]

def skip_ln(n, i):
    x, skip, gamma, beta = n.input[:4]
    p = f"u_sln{i}_"
    nodes, cur = [], x
    if len(n.input) > 4 and n.input[4]:
        nodes.append(h.make_node("Add", [x, n.input[4]], [p + "b"])); cur = p + "b"
    eps = next(a.f for a in n.attribute if a.name == "epsilon")
    return nodes + [
        h.make_node("Add", [cur, skip], [p + "s"]),
        h.make_node("LayerNormalization", [p + "s", gamma, beta], [n.output[0]], axis=-1, epsilon=eps),
    ]

def gelu(n, i):
    p = f"u_gelu{i}_"
    x, cur = n.input[0], n.input[0]
    nodes = []
    if len(n.input) > 1 and n.input[1]:
        nodes.append(h.make_node("Add", [x, n.input[1]], [p + "b"])); cur = p + "b"
    return nodes + [
        h.make_node("Mul", [cur, "u_rsqrt2"], [p + "d"]),
        h.make_node("Erf", [p + "d"], [p + "e"]),
        h.make_node("Add", [p + "e", "u_one"], [p + "e1"]),
        h.make_node("Mul", [cur, p + "e1"], [p + "m"]),
        h.make_node("Mul", [p + "m", "u_half"], [n.output[0]]),
    ]

for i, n in enumerate(g.node):
    if n.domain == "com.microsoft":
        out_nodes += {"Attention": attention, "SkipLayerNormalization": skip_ln, "FastGelu": gelu}[n.op_type](n, i)
    elif n.op_type == "ReduceSum" and n.output[0].startswith("mask_index"):
        continue
    elif n.op_type == "Cast" and n.output[0] == "attention_mask_int32":
        continue
    elif n.op_type == "Unsqueeze":
        axes = [a for a in n.attribute if a.name == "axes"][0].ints
        name = const(f"u_unsq{i}", np.array(list(axes), np.int64))
        out_nodes.append(h.make_node("Unsqueeze", [n.input[0], name], list(n.output)))
    elif n.op_type == "LayerNormalization":
        eps = next(a.f for a in n.attribute if a.name == "epsilon")
        out_nodes.append(h.make_node("LayerNormalization", list(n.input), list(n.output), axis=-1, epsilon=eps))
    else:
        out_nodes.append(n)

del g.node[:]
g.node.extend(out_nodes)
g.initializer.extend(extra_inits)
del g.value_info[:]
del m.opset_import[:]
m.opset_import.extend([h.make_opsetid("", 17)])
m.ir_version = 8
onnx.checker.check_model(m)
onnx.save(m, dst)
print({"nodes": len(g.node), "inputs": [(i.name, [d.dim_param or d.dim_value for d in i.type.tensor_type.shape.dim]) for i in g.input]})
