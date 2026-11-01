from .pricing import calculate_total


def create_quote(prices):
    total = calculate_total(prices)
    return {"subtotal_cents": total, "discount_cents": 0, "total_cents": total}
