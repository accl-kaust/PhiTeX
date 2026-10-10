# bench/flow-par/sdump.py: gdb command `sdump` (pgfstack.sh, SDUMP=1): in
# each thread, the innermost three frames of `Exec`'s methods with the
# node's sizes (its definitions, reads, steps, items emitted) and the
# piece's text length.
import gdb

class SDump(gdb.Command):
    def __init__(self):
        super().__init__("sdump", gdb.COMMAND_USER)

    def invoke(self, arg, tty):
        for th in gdb.selected_inferior().threads():
            th.switch()
            f = gdb.newest_frame()
            i = shown = 0
            while f is not None and i < 300 and shown < 3:
                n = f.name() or ''
                if any(n == m or n.endswith('::' + m) for m in ('def_in', 'expand_text', 'csname')):
                    f.select()
                    out = []
                    for e in ['self.defs.len', 'self.reads.len', 'self.steps', 'self.emit.len', 'self.text.length', 'self.in_csname']:
                        try:
                            out.append(f"{e.split('.',1)[1]}={gdb.parse_and_eval(e)}")
                        except Exception as x:
                            out.append(f"{e.split('.',1)[1]}=?({str(x)[:50]})")
                    print(f"SDUMP thread {th.num} frame {i} {n[-60:]}: " + ' '.join(out))
                    shown += 1
                f = f.older()
                i += 1

SDump()
