"""Offline tests for our own programs added in P1c (order pricing, VAT and shipping, insurance quote) and P1d (invoice
validation, refund policy): descriptors, examples against reference.py, and the held-out generators' ranges and
boundaries."""
import ast
import json
import sys
from decimal import Decimal as D
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).parent.parent))

import common  # noqa: E402
import score  # noqa: E402

PROGRAMS = ["order-pricing", "vat-shipping", "insurance-quote", "invoice-validation", "refund-policy"]


def whole(x, scale=1):
    x = D(x) * scale
    return x == x.to_integral_value()


def in_domain_order_pricing(customer_tier, unit_price, quantity, coupon):
    return (customer_tier in ("standard", "silver", "gold") and coupon in ("NONE", "SAVE15", "FLAT25")
            and whole(unit_price, 100) and D("0.50") <= unit_price <= 500 and whole(quantity) and 1 <= quantity <= 1000)


def in_domain_vat_shipping(region, category, net_amount, weight_kg):
    return (region in ("home", "eu", "uk", "world") and category in ("standard", "books", "food")
            and whole(net_amount, 100) and D("0.01") <= net_amount <= 5000
            and whole(weight_kg, 1000) and D("0.001") <= weight_kg <= 30)


def in_domain_insurance_quote(driver_age, vehicle_group, claims_last_5_years, annual_mileage, has_telematics):
    return (whole(driver_age) and 17 <= driver_age <= 90 and vehicle_group in ("A", "B", "C", "D")
            and whole(claims_last_5_years) and 0 <= claims_last_5_years <= 5
            and whole(annual_mileage) and 0 <= annual_mileage <= 60000 and isinstance(has_telematics, bool))


def in_domain_invoice_validation(region, net_amount, declared_tax, line_count, invoice_day, received_day,
                                 payment_terms_days, approver_role):
    return (region in ("DE", "NL", "UK", "CH") and approver_role in ("clerk", "manager", "director")
            and whole(net_amount, 100) and D("0.01") <= net_amount <= 250000
            and whole(declared_tax, 100) and 0 <= declared_tax <= 60000
            and whole(line_count) and 0 <= line_count <= 500
            and all(whole(d) and 1 <= d <= 730 for d in (invoice_day, received_day))
            and whole(payment_terms_days) and 0 <= payment_terms_days <= 120)


def in_domain_refund_policy(category, membership, days_since_purchase, condition, defective, price):
    return (category in ("electronics", "apparel", "furniture") and membership in ("none", "plus", "premier")
            and whole(days_since_purchase) and 0 <= days_since_purchase <= 365
            and condition in ("unopened", "opened", "used") and isinstance(defective, bool)
            and whole(price, 100) and 1 <= price <= 5000)


IN_DOMAIN = {"order-pricing": in_domain_order_pricing, "vat-shipping": in_domain_vat_shipping,
             "insurance-quote": in_domain_insurance_quote, "invoice-validation": in_domain_invoice_validation,
             "refund-policy": in_domain_refund_policy}
# invoice-validation's reasons are a list of codes, in order
LISTS = {"invoice-validation": {"reasons"}}


def args(prog, case):
    return [case[k] for k in prog["inputs"]]


@pytest.mark.parametrize("pid", PROGRAMS)
def test_descriptor_loads_and_names_its_files(pid):
    prog = common.load_program(pid)
    folder = common.program_dir(pid)
    assert prog["id"] == pid and prog["oracle"] == "python" and "dmn" not in prog
    for name in ("requirements.md", "signatures.py", "examples.json", "gen.py", "reference.py", prog["source"]):
        assert (folder / name).is_file(), name
    source = (folder / prog["source"]).read_text()
    assert source.startswith("language: velme/0.2\n") and f"goal {prog['goal']}(" in source
    defs = {n.name: [a.arg for a in n.args.args] for n in ast.parse((folder / "signatures.py").read_text()).body
            if isinstance(n, ast.FunctionDef)}
    assert defs == {prog["entry"]: prog["inputs"]}
    for f in prog["fields"]:
        if f["name"] in LISTS.get(pid, ()):
            assert f == {"name": f["name"], "compare": "list", "item": {"compare": "exact"}}
        else:
            assert f["compare"] == "exact", f


@pytest.mark.parametrize("pid", PROGRAMS)
def test_reference_reproduces_every_example_exactly(pid):
    prog = common.load_program(pid)
    ref = common.load_module(pid, "reference")
    examples = common.loads(common.program_path(pid, "examples.json").read_text())
    assert 5 <= len(examples) <= 8
    for ex in examples:
        assert ex["function"] == prog["entry"] and IN_DOMAIN[pid](*ex["args"]), ex
        got = getattr(ref, ex["function"])(*ex["args"])
        assert set(got) == set(common.field_names(prog))
        assert got == ex["expected"], ex
        # exact means the same digits too, not only numerically equal (examples are shown as written)
        assert json.loads(common.dumps(got)) == json.loads(common.dumps(ex["expected"])), ex


@pytest.mark.parametrize("pid", PROGRAMS)
def test_gen_yields_200_in_domain_cases_with_kinds(pid):
    prog = common.load_program(pid)
    cases = common.load_module(pid, "gen").generate(1, 200)
    assert [c["id"] for c in cases] == list(range(1, 201))
    assert all(IN_DOMAIN[pid](*args(prog, c)) for c in cases)
    kinds = [c["kind"] for c in cases]
    assert kinds.count("boundary") == 120 and kinds.count("random") == 80
    assert common.dumps(cases) == common.dumps(common.load_module(pid, "gen").generate(1, 200))  # seeded
    assert len({common.dumps(args(prog, c)) for c in cases if c["kind"] == "boundary"}) == 120


def brackets(values, target):
    """The generated values land on the target and on the cent below it."""
    return D(target) in values and D(target) - D("0.01") in values


def tie_raw(ref, c):
    """The unrounded tier and coupon discounts of an order-pricing case."""
    volume, _, d = ref.discounted_subtotal(c["customer_tier"], c["unit_price"], c["quantity"])
    raw = [(c["unit_price"] * c["quantity"] - volume) * ref.TIER_RATE[c["customer_tier"]]]
    return raw + ([d * D("0.15")] if c["coupon"] == "SAVE15" and d >= 100 else [])


def test_order_pricing_gen_lands_on_and_next_to_every_threshold():
    gen = common.load_module("order-pricing", "gen")
    ref = common.load_module("order-pricing", "reference")
    cases = gen.generate(1, 200)
    assert {1, 9, 10, 11, 49, 50, 51, 199, 200, 201, 1000} <= {int(c["quantity"]) for c in cases}
    assert {D("0.50"), D("500.00")} <= {c["unit_price"] for c in cases}
    disc = {(c["coupon"], ref.discounted_subtotal(c["customer_tier"], c["unit_price"], c["quantity"])[2]) for c in cases}
    assert brackets({d for cp, d in disc if cp == "SAVE15"}, "100.00")
    assert brackets({d for cp, d in disc if cp == "FLAT25"}, "200.00")
    save15 = {d for cp, d in disc if cp == "SAVE15"}
    # fix round 1, finding 4: the rounded 15% reaches 40.00 at 266.64, the unrounded 15% passes it at 266.67, the
    # rounded 15% at 266.70
    assert all(brackets(save15, t) for t in ("266.64", "266.67", "266.70"))
    priced = [ref.price_order(*[c[k] for k in ("customer_tier", "unit_price", "quantity", "coupon")])
              for c in cases if c["customer_tier"] != "gold"]
    merchandise = {p["total"] - p["shipping"] for p in priced}
    assert D("75.00") in merchandise and any(D("74.90") <= m < 75 for m in merchandise)  # free shipping from 75.00
    assert sum("tie" in c["tags"] for c in cases) >= 15
    # fix round 1, finding 3: the ties are ones half-up and half-even round differently
    assert sum(gen.disagrees(x) for c in cases if "tie" in c["tags"] for x in tie_raw(ref, c)) >= 15


def test_vat_shipping_gen_lands_on_and_next_to_every_threshold():
    gen = common.load_module("vat-shipping", "gen")
    ref = common.load_module("vat-shipping", "reference")
    cases = gen.generate(1, 200)
    weights = {c["weight_kg"] for c in cases}
    assert {D(w) for w in ("0.001", "0.499", "0.500", "0.501", "1.999", "2.000", "2.001", "9.999", "10.000", "10.001",
                           "30.000")} <= weights
    for region in ("home", "eu"):
        nets = {c["net_amount"] for c in cases if c["region"] == region}
        assert {D("49.99"), D("50.00"), D("50.01"), D("149.99"), D("150.00"), D("150.01")} <= nets, region
    # goods VAT ties where half-even and half-up round differently, and where they agree
    raw = [c["net_amount"] * ref.VAT[c["region"]][c["category"]] * 100 for c in cases]
    ties = [x for x in raw if x % 1 == D("0.5")]
    assert sum(int(x) % 2 == 0 for x in ties) >= 4 and sum(int(x) % 2 == 1 for x in ties) >= 4
    # every non-free shipping price of every region appears
    shipped = {(c["region"], ref.shipping_charge(c["region"], c["net_amount"], c["weight_kg"])) for c in cases}
    assert {(r, p) for r, prices in ref.SHIPPING.items() for p in prices} <= shipped


def test_insurance_quote_gen_lands_on_and_next_to_every_threshold():
    gen = common.load_module("insurance-quote", "gen")
    ref = common.load_module("insurance-quote", "reference")
    cases = gen.generate(1, 200)
    assert {17, 20, 21, 22, 24, 25, 26, 64, 65, 66, 90} <= {int(c["driver_age"]) for c in cases}
    assert {0, 1, 2, 3, 4, 5} == {int(c["claims_last_5_years"]) for c in cases}
    assert {0, 19999, 20000, 20001, 34999, 35000, 35001, 60000} <= {int(c["annual_mileage"]) for c in cases}
    net = [ref.before_limits(*[c[k] for k in ("driver_age", "vehicle_group", "claims_last_5_years", "annual_mileage",
                                              "has_telematics")])[3] for c in cases]
    # Premiums are discrete (the inputs only matter through their bands), so "next to" a limit is the nearest
    # premium any input reaches on each side; gen's grid holds every band combination.
    every = {ref.before_limits(*c)[3] for c in gen.grid()}
    for limit in (ref.MINIMUM, ref.CAP):
        assert max(x for x in every if x < limit) in net and min(x for x in every if x >= limit) in net, limit
    assert sum("tie" in c["tags"] for c in cases) >= 10
    # fix round 1, findings 2 and 3: ties half-up and half-even round differently, in the discount and in the
    # monthly instalment (on the annual premium after the minimum and cap)
    steps = [gen.ties(tuple(c[k] for k in ("driver_age", "vehicle_group", "claims_last_5_years", "annual_mileage",
                                           "has_telematics"))) for c in cases if "tie" in c["tags"]]
    assert sum("discount" in s for s in steps) >= 5 and sum("instalment" in s for s in steps) >= 5


def test_invoice_validation_scores_reasons_as_an_ordered_list_of_codes():
    prog = common.load_program("invoice-validation")
    want = {"accepted": False, "reasons": ["STALE", "OVERDUE"], "computed_tax": D("1.90"), "gross_total": D("11.90"),
            "manual_approval": False}
    assert score.score_case({"output": want}, want, prog["fields"]) == ([], False)
    for reasons in (["OVERDUE", "STALE"], ["STALE"], ["STALE", "OVERDUE", "OVERDUE"], "STALE, OVERDUE", [1, 2]):
        assert score.score_case({"output": {**want, "reasons": reasons}}, want, prog["fields"])[0] == ["reasons"], reasons
    accepted = {**want, "accepted": True, "reasons": []}
    assert score.score_case({"output": accepted}, accepted, prog["fields"]) == ([], False)
    assert score.score_case({"output": {**accepted, "reasons": None}}, accepted, prog["fields"])[0] == ["reasons"]


def invoice_args(c):
    return [c[k] for k in common.load_program("invoice-validation")["inputs"]]


def test_invoice_validation_gen_lands_on_and_next_to_every_threshold():
    gen = common.load_module("invoice-validation", "gen")
    ref = common.load_module("invoice-validation", "reference")
    cases = gen.generate(1, 200)
    assert {0, 1, 2, 199, 200, 201, 500} <= {int(c["line_count"]) for c in cases}
    gaps = {int(c["received_day"] - c["invoice_day"]) for c in cases}
    assert {-1, 0, 1, 179, 180, 181} <= gaps
    assert {0, 59, 60, 61, 120} <= {int(c["payment_terms_days"]) for c in cases}
    assert {-1, 0, 1} <= {int(c["invoice_day"] + c["payment_terms_days"] - c["received_day"]) for c in cases}
    # fix round 1, finding 3: an overdue case one day late is not also future dated, on any seed
    for seed in range(1, 40):
        for c in gen.generate(seed, 200):
            if "overdue:-1" in c["tags"]:
                assert in_domain_invoice_validation(*invoice_args(c)) and c["received_day"] >= c["invoice_day"], (seed, c)
    assert {1, 730} <= {int(c["invoice_day"]) for c in cases} & {int(c["received_day"]) for c in cases}
    assert {D("0.01"), D("250000.00")} <= {c["net_amount"] for c in cases}
    diffs = {c["declared_tax"] - ref.computed_tax(c["region"], c["net_amount"]) for c in cases}
    assert {D(x) for x in ("-0.03", "-0.02", "-0.01", "0.01", "0.02", "0.03")} <= diffs
    out = [ref.validate_invoice(*invoice_args(c)) for c in cases]
    gross = {o["gross_total"] for o in out}
    for limit in ("5000.00", "50000.00", "200000.00"):
        assert any(D(limit) - 1 < x <= D(limit) for x in gross) and any(D(limit) < x < D(limit) + 1 for x in gross), limit
    for role, limit in (("clerk", D(5000)), ("manager", D(50000))):
        # an accepted invoice either side of the approver's limit, so manual approval flips there
        near = [o for c, o in zip(cases, out) if c["approver_role"] == role and o["accepted"]
                and abs(o["gross_total"] - limit) < 1]
        assert {o["manual_approval"] for o in near} == {True, False}, role
    # every reason both fails and passes somewhere, and accepted invoices are common
    for code in ref.REASONS:
        assert 0 < sum(code in o["reasons"] for o in out) < 200, code
    assert sum(o["accepted"] for o in out) >= 40
    # ties half-up and half-even round differently, with the declared tax at the tolerance's edges
    ties = [c for c in cases if "tie" in c["tags"]]
    assert len(ties) >= 10 and all(gen.disagrees(c["region"], int(c["net_amount"] * 100)) for c in ties)
    half_up = [(c["net_amount"] * ref.TAX_RATE[c["region"]]).quantize(D("0.01"), "ROUND_HALF_UP") for c in ties]
    flips = [c for c, h in zip(ties, half_up)
             if (abs(c["declared_tax"] - h) > ref.TOLERANCE) != ("TAX_MISMATCH" in ref.validate_invoice(*invoice_args(c))["reasons"])]
    assert len(flips) >= 3


def test_refund_policy_gen_lands_on_and_next_to_every_threshold():
    gen = common.load_module("refund-policy", "gen")
    ref = common.load_module("refund-policy", "reference")
    cases = gen.generate(1, 200)
    seen = {(c["category"], c["membership"], int(c["days_since_purchase"])) for c in cases if not c["defective"]}
    for cat in ref.WINDOW:
        for mem in ref.EXTENSION:
            w = ref.window_days(cat, mem)
            assert {(cat, mem, d) for d in (w - 1, w, w + 1, w + 29, w + 30, w + 31)} <= seen, (cat, mem)
    assert {179, 180, 181} <= {int(c["days_since_purchase"]) for c in cases if c["defective"]}
    assert {0, 365} <= {int(c["days_since_purchase"]) for c in cases}
    assert {D("1.00"), D("5000.00")} <= {c["price"] for c in cases}
    opened = [c for c in cases if c["condition"] == "opened"
              and c["days_since_purchase"] <= ref.window_days(c["category"], c["membership"])
              and not (c["defective"] and c["days_since_purchase"] <= 180)]
    for cat, mem in [(cat, mem) for cat in ref.WINDOW for mem in ref.EXTENSION if ref.fee_rate(cat, mem) > 0]:
        fees = {ref.cents(c["price"] * ref.fee_rate(cat, mem)) for c in opened
                if (c["category"], c["membership"]) == (cat, mem)}
        # the rounded fee just below, on and just past the 75.00 cap
        assert any(x < 75 for x in fees) and D("75.00") in fees and any(x > 75 for x in fees), (cat, mem)
    assert any(ref.fee_rate(c["category"], c["membership"]) == 0 for c in opened)  # an opened item with no fee
    ties = [c for c in cases if "tie" in c["tags"]]
    raw = [c["price"] * (ref.fee_rate(c["category"], c["membership"]) if c["condition"] == "opened" else ref.USED_CREDIT)
           for c in ties]
    assert len(ties) >= 15 and all(gen.disagrees(x) for x in raw)
    assert sum(c["condition"] == "used" for c in ties) >= 5
    # fix round 1, findings 1 and 2: the opened ties still differ after the 75.00 cap
    capped = [(min(x.quantize(D("0.01"), "ROUND_HALF_UP"), ref.FEE_CAP), min(x.quantize(D("0.01"), "ROUND_HALF_EVEN"), ref.FEE_CAP))
              for c, x in zip(ties, raw) if c["condition"] == "opened"]
    assert sum(up != even for up, even in capped) >= 5
    # fix round 1, finding 4: no apparel window case is used (it gets none on both sides of the window)
    assert not [c for c in cases if c["category"] == "apparel" and c["condition"] == "used"
                and any(t.startswith("window:") for t in c["tags"])]
    kinds = {ref.refund(*[c[k] for k in common.load_program("refund-policy")["inputs"]])["refund_type"] for c in cases}
    assert kinds == {"full", "partial", "store_credit", "none"}
