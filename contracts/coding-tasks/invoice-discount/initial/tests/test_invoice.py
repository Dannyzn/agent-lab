import json
import subprocess
import sys
import unittest

from invoice.pricing import calculate_total
from invoice.service import create_quote


class InvoiceTests(unittest.TestCase):
    def test_default_behavior_is_compatible(self):
        self.assertEqual(calculate_total([1000, 2000]), 3000)
        self.assertEqual(create_quote([1000, 2000]), {
            "subtotal_cents": 3000, "discount_cents": 0, "total_cents": 3000})

    def test_pricing_accepts_discount(self):
        self.assertEqual(calculate_total([1000, 2000], discount_percent=15), 2550)

    def test_service_propagates_discount(self):
        self.assertEqual(create_quote([1000, 2000], discount_percent=15), {
            "subtotal_cents": 3000, "discount_cents": 450, "total_cents": 2550})

    def test_invalid_discount_is_rejected(self):
        for percent in [-1, 101]:
            with self.assertRaises(ValueError):
                calculate_total([1000], discount_percent=percent)

    def test_cli_propagates_discount(self):
        result = subprocess.run([sys.executable, "-B", "-m", "invoice.cli", "--discount-percent", "15", "1000", "2000"],
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout), {
            "subtotal_cents": 3000, "discount_cents": 450, "total_cents": 2550})
