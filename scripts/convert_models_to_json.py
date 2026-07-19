#!/usr/bin/env python3
"""
Convert TypeScript models.generated.ts to a JSON array of model objects.

Usage: python3 scripts/convert_models_to_json.py <models.generated.ts> > models.json
"""

import json
import re
import sys


def parse_ts_value(text, pos):
    """Parse a TypeScript literal value starting at pos. Returns (value, new_pos)."""
    # Skip whitespace
    m = re.match(r'\s*', text[pos:])
    pos += m.end()

    if pos >= len(text):
        raise ValueError(f"Unexpected end at position {pos}")

    ch = text[pos]

    if ch == '{':
        return parse_ts_object(text, pos)
    elif ch == '[':
        return parse_ts_array(text, pos)
    elif ch in '"\'':
        quote = ch
        end = text.index(quote, pos + 1)
        return text[pos + 1:end], end + 1
    elif text.startswith('true', pos):
        return True, pos + 4
    elif text.startswith('false', pos):
        return False, pos + 5
    elif text.startswith('null', pos):
        return None, pos + 4
    else:
        # number or identifier
        m2 = re.match(r'(-?\d+(?:\.\d+)?)', text[pos:])
        if m2:
            s = m2.group(1)
            if '.' in s:
                return float(s), pos + len(s)
            return int(s), pos + len(s)
        # identifier (treat as string for now)
        m2 = re.match(r'([a-zA-Z_]\w*)', text[pos:])
        if m2:
            return m2.group(1), pos + m2.end()
        raise ValueError(f"Cannot parse at position {pos}: {text[pos:pos+60]!r}")


def parse_ts_object(text, pos):
    """Parse { key: value, ... } starting at pos (which must be '{')."""
    assert text[pos] == '{', f"Expected '{{' at {pos}, got {text[pos]!r}"
    pos += 1
    obj = {}
    while pos < len(text):
        # Skip whitespace and commas
        m = re.match(r'[\s,]+', text[pos:])
        if m:
            pos += m.end()
        if pos >= len(text):
            break
        if text[pos] == '}':
            return obj, pos + 1
        # Parse key (identifier or quoted string)
        key_m = re.match(r'([a-zA-Z_]\w*|"[^"]*"|\'[^\']*\')', text[pos:])
        if not key_m:
            raise ValueError(f"Cannot parse key at {pos}: {text[pos:pos+60]!r}")
        key = key_m.group(1).strip('"\'')
        pos += key_m.end()
        # Skip ':'
        colon = text.find(':', pos)
        if colon == -1:
            raise ValueError(f"No colon after key '{key}' at {pos}")
        pos = colon + 1
        # Parse value
        val, pos = parse_ts_value(text, pos)
        obj[key] = val
    return obj, pos


def parse_ts_array(text, pos):
    """Parse [value, ...] starting at pos (which must be '[')."""
    assert text[pos] == '['
    pos += 1
    arr = []
    while pos < len(text):
        m = re.match(r'[\s,]+', text[pos:])
        if m:
            pos += m.end()
        if pos >= len(text):
            break
        if text[pos] == ']':
            return arr, pos + 1
        val, pos = parse_ts_value(text, pos)
        # Handle TypeScript 'satisfies' keyword
        m2 = re.match(r'\s+satisfies\s+', text[pos:])
        if m2:
            pos += m2.end()
            # Skip the type expression until comma/brace/bracket
            m3 = re.match(r'[^,}\]\n]+', text[pos:])
            if m3:
                pos += m3.end()
        arr.append(val)
    return arr, pos


def main():
    if len(sys.argv) < 2:
        print(f"Usage: {sys.argv[0]} <models.generated.ts>", file=sys.stderr)
        sys.exit(1)

    with open(sys.argv[1]) as f:
        content = f.read()

    # Find the MODELS const export
    idx = content.find('export const MODELS = {')
    if idx == -1:
        print("Could not find 'export const MODELS ='", file=sys.stderr)
        sys.exit(1)

    brace = content.index('{', idx)
    models_obj, _ = parse_ts_object(content, brace)

    # Flatten to an array of model objects with provider added
    result = []
    for provider, models_dict in models_obj.items():
        if not isinstance(models_dict, dict):
            continue
        for model_id, model_obj in models_dict.items():
            if not isinstance(model_obj, dict):
                continue
            model_obj['provider'] = provider
            result.append(model_obj)

    json.dump(result, sys.stdout, indent=2, ensure_ascii=False)
    print()


if __name__ == '__main__':
    main()
