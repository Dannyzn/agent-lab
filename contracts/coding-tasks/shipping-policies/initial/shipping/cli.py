import argparse
import json

from shipping.pricing import shipping_cost


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--zone", required=True)
    parser.add_argument("--weight-grams", required=True, type=int)
    parser.add_argument("--subtotal-cents", default=0, type=int)
    args = parser.parse_args()
    # Replace the placeholder with a real quote and validation.
    print(json.dumps({"shipping_cents": None}))


if __name__ == "__main__":
    main()
