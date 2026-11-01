"""Host-controlled oracle. Not copied into the model's workspace."""
import json
import subprocess
import sys

from shipping.pricing import shipping_cost

RATES = {"north": (350, 90, 5000), "south": (420, 110, 7000),
         "east": (300, 80, 4500), "west": (390, 95, 6500),
         "central": (280, 70, 4000), "islands": (800, 240, 12000)}
count = 0
for zone, (base, extra, threshold) in RATES.items():
    for weight in [1, 499, 500, 501, 999, 1000, 1001, 10000]:
        for subtotal in [0, threshold - 1, threshold, threshold + 1]:
            expected = 0 if subtotal >= threshold else base + max(0, (weight - 500 + 499) // 500) * extra
            value = shipping_cost(weight, zone, subtotal)
            assert type(value) is int and value == expected, (zone, weight, subtotal, value, expected)
            count += 1
for weight, zone, subtotal in [(0, 'north', 0), (-1, 'north', 0), (True, 'north', 0),
                               (1.2, 'north', 0), ('500', 'north', 0), (None, 'north', 0),
                               (500, 'north', -1), (500, 'north', True), (500, 'north', 1.5),
                               (500, 'missing', 0), (500, [], 0), (0, 'north', 100000)]:
    try:
        shipping_cost(weight, zone, subtotal)
    except ValueError:
        pass
    else:
        raise AssertionError('invalid input accepted')
for zone, (base, _, threshold) in RATES.items():
    for subtotal, expected in [(0, base), (threshold, 0)]:
        result = subprocess.run([sys.executable, '-B', '-m', 'shipping.cli', '--zone', zone,
                                 '--weight-grams', '500', '--subtotal-cents', str(subtotal)],
                                check=True, capture_output=True, text=True)
        assert json.loads(result.stdout) == {'shipping_cents': expected}
for args in [['--zone','missing','--weight-grams','500'], ['--zone','north','--weight-grams','0'],
             ['--zone','north','--weight-grams','500','--subtotal-cents','-1']]:
    result = subprocess.run([sys.executable, '-B', '-m', 'shipping.cli'] + args, capture_output=True, text=True)
    assert result.returncode != 0
print(f'independent shipping checks passed: {count} rate/threshold cases, 12 invalid API cases, 12 valid CLI cases, 3 invalid CLI cases')
