# Makes qr.fixtures.json, the reference matrices for qr.test.ts, with two
# encoders independent of ours. Not dependencies: run it by hand when the
# texts change, with both from nixpkgs:
#
#   bun -e 'import {qrTexts} from "./apps/console/src/console/qrTexts.ts"; console.log(JSON.stringify(qrTexts()))' \
#     | nix shell --impure --expr 'let p = import (builtins.getFlake "nixpkgs") {}; in p.python3.withPackages (q: [q.qrcode q.qrcodegen])' \
#         -c python3 -I apps/console/src/console/qr-fixtures.py > apps/console/src/console/qr.fixtures.json
#
# The modules come from `qrcode` (8.2) with the mask forced; the mask the
# standard's penalty chooses comes from Project Nayuki's `qrcodegen` (1.8),
# whose penalty follows ISO/IEC 18004 exactly (`qrcode`'s and libqrencode's
# differ from it, and choose other masks for some texts). Each entry: the
# text's index, whether the mask was forced (else it is the one the penalty
# chooses), the version, the mask and the rows (hex, a bit per module, most
# significant first).
import json
import sys

import qrcode
from qrcode.util import MODE_8BIT_BYTE, QRData
from qrcodegen import QrCode, QrSegment

texts = json.load(sys.stdin)
out = []


def entry(i, mask, forced):
    data = texts[i].encode("utf-8")
    q = qrcode.QRCode(error_correction=qrcode.constants.ERROR_CORRECT_M, border=0, mask_pattern=mask)
    q.add_data(QRData(data, mode=MODE_8BIT_BYTE))
    q.make(fit=True)
    rows = []
    for row in q.get_matrix():
        bits = "".join("1" if m else "0" for m in row)
        bits += "0" * (-len(bits) % 4)
        rows.append("%0*x" % (len(bits) // 4, int(bits, 2)))
    return {"text": i, "forced": forced, "version": q.version, "mask": mask, "rows": rows}


for i, text in enumerate(texts):
    chosen = QrCode.encode_segments([QrSegment.make_bytes(text.encode("utf-8"))], QrCode.Ecc.MEDIUM, boostecl=False)
    out.append(entry(i, chosen.get_mask(), False))
for mask in range(8):
    out.append(entry(3, mask, True))
json.dump(out, sys.stdout, indent=0)
sys.stdout.write("\n")
