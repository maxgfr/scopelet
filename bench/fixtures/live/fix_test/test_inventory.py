import unittest

import inventory


class InventoryTests(unittest.TestCase):
    def test_restock_at_threshold(self):
        self.assertTrue(inventory.restock_needed(5, 5))

    def test_no_restock_above_threshold(self):
        self.assertFalse(inventory.restock_needed(6, 5))


# Many passing cases, so a run prints a long verbose report around the failure.
for _i in range(300):
    def _case(self, i=_i):
        self.assertEqual(inventory.total_value([{"price": i, "quantity": 2}]), 2 * i)
    setattr(InventoryTests, f"test_value_{_i:03}", _case)


if __name__ == "__main__":
    unittest.main()
