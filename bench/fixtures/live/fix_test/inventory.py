"""A tiny inventory model with one off-by-one bug."""


def restock_needed(quantity, threshold):
    """True when stock is at or below the threshold."""
    return quantity < threshold


def total_value(items):
    return sum(item["price"] * item["quantity"] for item in items)
