import unittest

from calculator import add


class CalculatorTest(unittest.TestCase):
    def test_adds_positive_integers(self):
        self.assertEqual(add(2, 3), 5)

    def test_adds_negative_integers(self):
        self.assertEqual(add(-2, -3), -5)
