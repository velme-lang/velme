"""Offline tests for our own programs added in P1c (order pricing, VAT and shipping, insurance quote): descriptors,
examples against reference.py, and the held-out generators' ranges and boundaries."""
import ast
import json
import sys
from decimal import Decimal as D
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).parent.parent))

import common  # noqa: E402

PROGRAMS = ["order-pricing", "vat-shipping", "insurance-quote"]


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


IN_DOMAIN = {"order-pricing": in_domain_order_pricing, "vat-shipping": in_domain_vat_shipping,
             "insurance-quote": in_domain_insurance_quote}


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
    assert all(f["compare"] == "exact" for f in prog["fields"])


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
