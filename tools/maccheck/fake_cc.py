import sys, pathlib
a = sys.argv[1:]
if a and a[0] == 'ar':
    # ar <flags> <archive> <objects...>
    for x in a[2:3]:
        pathlib.Path(x).write_bytes(b"!<arch>\n")
    sys.exit(0)
if '-o' in a:
    pathlib.Path(a[a.index('-o') + 1]).write_bytes(b'')
sys.exit(0)
