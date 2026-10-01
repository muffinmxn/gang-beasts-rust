"""One off-screen render and score, reusing the comparison harness and a named output.

python tools/harness/check.py --out target/harness/before.png
No timestamped copies; repeat runs overwrite the requested capture and heatmap.
"""
import argparse
import json
import os
from tune import render
from compare import compare


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--ref', default='referen/harness/title_1080.png')
    parser.add_argument('--screen', default='splash')
    parser.add_argument('--out', default='target/harness/after.png')
    parser.add_argument('--params', help='JSON object of environment overrides')
    args = parser.parse_args()
    os.makedirs(os.path.dirname(os.path.abspath(args.out)), exist_ok=True)
    params = json.loads(args.params) if args.params else {}
    if not render(params, args.screen, os.path.abspath(args.out)):
        raise SystemExit('Render failed: no capture written')
    result = compare(args.ref, args.out)
    result.pop('de')
    print(json.dumps(result, indent=2))
    with open(os.path.splitext(args.out)[0] + '.json', 'w') as stream:
        json.dump({'reference': args.ref, 'screen': args.screen, 'params': params,
                   'metrics': result}, stream, indent=2)


if __name__ == '__main__':
    main()
