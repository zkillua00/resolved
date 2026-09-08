#!/usr/bin/env python3
"""Compare compiled grammar lookups and parsing before/after table compaction.

Run after scripts/cargo.sh build --release. Requires Python 3 and a C compiler.
All generated verification artifacts stay in a temporary directory.
"""
import argparse
import ctypes as C
import json
import os
from pathlib import Path
import statistics
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
GRAMMARS = {
    'c-sharp': ('0.23.5', 'c_sharp', 'class Example { public int Add(int x) { return x + 1; } }\n'),
    'cpp': ('0.23.4', 'cpp', 'template<class T> T add(T x) { return x + 1; }\n'),
    'swift': ('0.7.3', 'swift', 'struct Example { func add(_ x: Int) -> Int { return x + 1 } }\n'),
    'kotlin-ng': ('1.1.0', 'kotlin', 'fun add(x: Int): Int = x + 1\n'),
    'scala': ('0.23.4', 'scala', 'object Example { def add(x: Int): Int = { x + 1 } }\n'),
    'sequel': ('0.3.11', 'sql', 'SELECT name, count(*) FROM users WHERE id > 1 GROUP BY name;\n'),
}


def run(args):
    result = subprocess.run([str(a) for a in args], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if result.returncode:
        raise RuntimeError(result.stderr.decode(errors='replace'))


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--build-dir', type=Path, default=ROOT / 'target/release')
    ap.add_argument('--output', type=Path)
    args = ap.parse_args()
    cargo_home = Path(os.environ.get('CARGO_HOME', Path.home() / '.cargo'))
    engine = next((cargo_home / 'registry/src').glob('*/tree-sitter-0.25.10'))
    results = []
    with tempfile.TemporaryDirectory(prefix='resolved-grammar-check-') as temporary:
        tmp = Path(temporary)
        engine_o = tmp / 'engine.o'
        run(['cc', '-O2', '-fPIC', '-std=c11', '-D_DEFAULT_SOURCE', '-I', engine/'lib/include' if (engine/'lib').exists() else engine/'include', '-I', engine/'src', '-c', engine/'src/lib.c', '-o', engine_o])
        for name, (version, symbol, snippet) in GRAMMARS.items():
            source = ROOT / f'vendor/tree-sitter-{name}-{version}/src'
            generated = list((args.build_dir/'build').glob(f'tree-sitter-{name}-*/out/parser-compact.c'))
            assert generated, f'Build compact tree-sitter-{name} first'
            compact = max(generated, key=lambda p: p.stat().st_mtime)
            objects = []
            for variant, parser in [('dense', source/'parser.c'), ('compact', compact)]:
                wrapper = tmp / f'{name}-{variant}.c'
                wrapper.write_text(f'''#define tree_sitter_{symbol} grammar_{variant}
#include "{parser}"
unsigned states_{variant}(void) {{ return STATE_COUNT; }}
unsigned symbols_{variant}(void) {{ return SYMBOL_COUNT; }}
void dump_{variant}(uint16_t *out) {{
  const TSLanguage *l = grammar_{variant}();
  for (unsigned s = 0; s < STATE_COUNT; ++s) {{
    for (unsigned t = 0; t < SYMBOL_COUNT; ++t) {{
      unsigned value = 0;
      if (s < l->large_state_count) value = l->parse_table[s * SYMBOL_COUNT + t];
      else {{
        const uint16_t *p = l->small_parse_table + l->small_parse_table_map[s-l->large_state_count];
        unsigned groups = *p++;
        for (unsigned g = 0; g < groups; ++g) {{
          unsigned v = *p++, n = *p++;
          for (unsigned j = 0; j < n; ++j) if (*p++ == t) value = v;
        }}
      }}
      *out++ = value;
    }}
  }}
}}
''')
                obj = wrapper.with_suffix('.o')
                run(['cc', '-O2', '-fPIC', '-w', '-I', source, '-c', wrapper, '-o', obj])
                objects.append(obj)
            scanner = tmp / f'{name}-scanner.o'
            run(['cc', '-O2', '-fPIC', '-w', '-I', source, '-c', source/'scanner.c', '-o', scanner])
            lib_path = tmp / f'{name}.dylib'
            run(['cc', '-shared', *objects, scanner, engine_o, '-o', lib_path])
            lib = C.CDLL(str(lib_path))
            states, symbols = lib.states_dense(), lib.symbols_dense()
            assert (states, symbols) == (lib.states_compact(), lib.symbols_compact())
            arrays = [(C.c_uint16 * (states*symbols))() for _ in range(2)]
            lib.dump_dense(arrays[0]); lib.dump_compact(arrays[1])
            assert bytes(arrays[0]) == bytes(arrays[1]), f'{name}: table mismatch'
            lib.ts_parser_new.restype = C.c_void_p
            lib.ts_parser_set_language.argtypes = [C.c_void_p, C.c_void_p]
            lib.ts_parser_set_language.restype = C.c_bool
            lib.ts_parser_parse_string.argtypes = [C.c_void_p, C.c_void_p, C.c_char_p, C.c_uint32]
            lib.ts_parser_parse_string.restype = C.c_void_p
            lib.ts_tree_delete.argtypes = [C.c_void_p]
            lib.ts_parser_delete.argtypes = [C.c_void_p]
            class Node(C.Structure):
                _fields_ = [('context', C.c_uint32 * 4), ('id', C.c_void_p), ('tree', C.c_void_p)]
            class Point(C.Structure):
                _fields_ = [('row', C.c_uint32), ('column', C.c_uint32)]
            class Edit(C.Structure):
                _fields_ = [('start_byte', C.c_uint32), ('old_end_byte', C.c_uint32), ('new_end_byte', C.c_uint32), ('start_point', Point), ('old_end_point', Point), ('new_end_point', Point)]
            lib.ts_tree_root_node.argtypes = [C.c_void_p]; lib.ts_tree_root_node.restype = Node
            lib.ts_node_string.argtypes = [Node]; lib.ts_node_string.restype = C.c_void_p
            lib.ts_node_has_error.argtypes = [Node]; lib.ts_node_has_error.restype = C.c_bool
            lib.ts_tree_edit.argtypes = [C.c_void_p, C.POINTER(Edit)]
            libc = C.CDLL(None); libc.free.argtypes = [C.c_void_p]
            timings = {}; trees = {}; incremental = {}
            for variant in ['dense', 'compact']:
                language = getattr(lib, 'grammar_' + variant); language.restype = C.c_void_p
                parser = lib.ts_parser_new(); assert lib.ts_parser_set_language(parser, language())
                sexps = []
                for index, text in enumerate([snippet, snippet[:-8], snippet.replace('+', '('), snippet*300]):
                    data = text.encode(); tree = lib.ts_parser_parse_string(parser, None, data, len(data)); assert tree
                    if index == 0:
                        assert not lib.ts_node_has_error(lib.ts_tree_root_node(tree)), f'{name}: invalid benchmark fixture'
                    ptr = lib.ts_node_string(lib.ts_tree_root_node(tree)); sexps.append(C.string_at(ptr)); libc.free(ptr)
                    lib.ts_tree_delete(tree)
                trees[variant] = sexps
                data = (snippet*300).encode()
                samples = []
                for _ in range(15):
                    start = time.perf_counter()
                    tree = lib.ts_parser_parse_string(parser, None, data, len(data))
                    samples.append(time.perf_counter()-start); lib.ts_tree_delete(tree)
                timings[variant] = statistics.median(samples)
                tree = lib.ts_parser_parse_string(parser, None, data, len(data))
                position = data.index(b'1'); row = data[:position].count(b'\n'); col = position-(data.rfind(b'\n', 0, position)+1)
                edit = Edit(position, position+1, position+1, Point(row,col), Point(row,col+1), Point(row,col+1))
                samples = []
                for i in range(30):
                    changed = data[:position] + (b'2' if i%2 == 0 else b'1') + data[position+1:]
                    lib.ts_tree_edit(tree, C.byref(edit))
                    start = time.perf_counter(); new = lib.ts_parser_parse_string(parser, tree, changed, len(changed)); samples.append(time.perf_counter()-start)
                    lib.ts_tree_delete(tree); tree = new
                ptr = lib.ts_node_string(lib.ts_tree_root_node(tree)); trees[variant].append(C.string_at(ptr)); libc.free(ptr)
                incremental[variant] = statistics.median(samples)
                lib.ts_tree_delete(tree); lib.ts_parser_delete(parser)
            assert trees['dense'] == trees['compact'], f'{name}: parse tree mismatch'
            result = dict(language=name, lookups=states*symbols, full_ms={k:v*1000 for k,v in timings.items()}, incremental_ms={k:v*1000 for k,v in incremental.items()})
            results.append(result); print(json.dumps(result), flush=True)
    if args.output: args.output.write_text(json.dumps(results, indent=2)+'\n')

if __name__ == '__main__':
    main()
