import json
import subprocess
import sys

from invoice.pricing import calculate_total
from invoice.service import create_quote

cases = [([], 0), ([1], 50), ([1, 2], 33), ([123, 456, 789], 17),
         ([999], 0), ([999], 100), ([0, 0], 99), ([10**12, 7], 3)]
for prices, percent in cases:
    original = list(prices)
    subtotal = sum(prices)
    total = subtotal * (100 - percent) // 100
    assert calculate_total(prices, discount_percent=percent) == total, (prices, percent)
    assert create_quote(prices, discount_percent=percent) == {
        'subtotal_cents': subtotal, 'discount_cents': subtotal - total, 'total_cents': total}
    assert prices == original, 'input list was mutated'
for percent in [-100, -1, 101, 1000]:
    for function in [calculate_total, create_quote]:
        try:
            function([123], discount_percent=percent)
        except ValueError:
            pass
        else:
            raise AssertionError('invalid discount accepted: {}'.format(percent))
for percent, prices in [(0, [3, 4]), (100, [999]), (33, [1, 2])]:
    output = subprocess.run([sys.executable, '-B', '-m', 'invoice.cli', '--discount-percent', str(percent)] + [str(p) for p in prices],
                            capture_output=True, text=True, check=True)
    assert json.loads(output.stdout) == create_quote(prices, discount_percent=percent)
for percent in [-1, 101]:
    output = subprocess.run([sys.executable, '-B', '-m', 'invoice.cli', '--discount-percent', str(percent), '100'],
                            capture_output=True, text=True)
    assert output.returncode != 0
print('independent invoice checks: 8 data cases, 8 invalid API calls, 3 valid CLI cases, 2 invalid CLI cases passed')
