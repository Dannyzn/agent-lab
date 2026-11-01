import unittest

from shipping.pricing import shipping_cost


class ShippingTests(unittest.TestCase):
    def test_basic_quote(self):
        self.assertEqual(shipping_cost(500, "north"), 350)

    def test_free_threshold(self):
        self.assertEqual(shipping_cost(1000, "north", 5000), 0)

    def test_invalid_weight(self):
        with self.assertRaises(ValueError):
            shipping_cost(0, "north")
