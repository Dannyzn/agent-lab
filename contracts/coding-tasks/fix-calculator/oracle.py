from calculator import add

pairs = [(0, 0), (-5, 2), (7, -9), (10**12, 1), (-100, 0), (0, -3)]
for left, right in pairs:
    assert add(left, right) == left + right, (left, right)
print('independent calculator checks: 6 passed')
