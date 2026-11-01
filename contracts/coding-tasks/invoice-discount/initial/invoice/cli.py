import argparse
import json

from .service import create_quote


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("prices", nargs="+", type=int)
    args = parser.parse_args()
    print(json.dumps(create_quote(args.prices), sort_keys=True))


if __name__ == "__main__":
    main()
