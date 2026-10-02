"""Generate the held-out inputs: seeded, ~60% boundary cases from requirements.md, ~40% uniform random.

Usage: python gen_inputs.py [--seed 1] [--n 200] [--out 0087/heldout/inputs.jsonl]
Money is written to the cent, rate to 4 places, as exact decimal text in the JSON (never via float).
"""
import argparse
import itertools
import random
import sys
from decimal import Decimal as D
from pathlib import Path

HERE = Path(__file__).parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE / "0087"))
import reference as ref  # noqa: E402
from common import dumps  # noqa: E402

CENT = D("0.01")
AGE_PTS = [(18, 32), (21, 32), (22, 35), (25, 35), (26, 40), (35, 40), (36, 43), (49, 43), (50, 48), (80, 48)]
MARITAL = ["S", "M"]
EMPLOYMENT = ["UNEMPLOYED", "STUDENT", "EMPLOYED", "SELF-EMPLOYED"]
AGE_EDGES = [18, 19, 21, 22, 25, 26, 35, 36, 49, 50]
# Score edges: the first score of the next band and the last of the one below, per table (new, existing).
PRE_EDGES = {False: [100, 120, 130, 131], True: [80, 90, 110, 111]}
POST_EDGES = {False: [120, 130, 131], True: [100, 101]}
# credit-score edges per (existing, score band) table of the post-bureau category: (lo, hi) of MEDIUM
CREDIT_TABLES = [(False, 0, 119, 590, 610), (False, 120, 130, 600, 625), (True, 0, 100, 580, 600), (True, 101, 999, 590, 615)]


def points(age, marital, emp):
    return int(ref.application_risk_score({"age": age, "marital_status": marital, "employment_status": emp}))


COMBOS = [(a, m, e, points(a, m, e)) for a in range(18, 81) for m in MARITAL for e in EMPLOYMENT]
REACHABLE = sorted({c[3] for c in COMBOS})


def near(target):
    """Reachable sums: target itself if reachable, else nearest below and above; plus nearest each side."""
    below = max((s for s in REACHABLE if s < target), default=None)
    above = min((s for s in REACHABLE if s > target), default=None)
    return [s for s in (below, target if target in REACHABLE else None, above) if s is not None]


def cents(rng, lo=0, hi=50000):
    return D(rng.randint(lo * 100, hi * 100)).scaleb(-2)


def rand_applicant(rng, **fix):
    a = {"age": rng.randint(18, 80), "marital_status": rng.choice(MARITAL), "employment_status": rng.choice(EMPLOYMENT),
         "existing_customer": rng.random() < 0.5, "monthly_income": cents(rng), "monthly_repayments": cents(rng),
         "monthly_expenses": cents(rng)}
    a.update(fix)
    return a


def rand_product(rng):
    amount = D(0) if rng.random() < 0.05 else cents(rng, 0, 500000)
    return {"product_type": rng.choice(["STANDARD LOAN", "SPECIAL LOAN"]), "rate": D(rng.randint(100, 2500)).scaleb(-4),
            "term": rng.randint(1, 360), "amount": amount.quantize(CENT)}


def rand_bureau(rng, **fix):
    b = {"bankrupt": rng.random() < 0.10, "credit_score": rng.randint(0, 999)}
    b.update(fix)
    return b


def applicant_with_score(rng, score, existing):
    a, m, e, _ = rng.choice([c for c in COMBOS if c[3] == score])
    return rand_applicant(rng, age=a, marital_status=m, employment_status=e, existing_customer=existing)


def afford_case(rng, k):
    """Applicant whose disposable income sits one cent either side of the affordability threshold."""
    above = k % 2 == 0
    stage = "post" if (k // 2) % 2 else "pre"
    while True:
        applicant, product, bureau = rand_applicant(rng), rand_product(rng), rand_bureau(rng)
        if k % 7 == 0:  # amount 0: the installment is exactly the fee, so equality is reachable with factor 0.8
            product["amount"] = D("0.00")
        score = ref.application_risk_score(applicant)
        category = ref.pre_bureau_risk_category(applicant["existing_customer"], score)
        if stage == "post":
            category = ref.post_bureau_risk_category(applicant["existing_customer"], score, bureau["credit_score"])
        factor, inst = ref.FACTOR[category], ref.required_monthly_installment(product)
        costs = applicant["monthly_repayments"] + applicant["monthly_expenses"]
        below = (inst / factor).quantize(CENT, rounding="ROUND_FLOOR")  # largest cent with below * factor <= inst
        while below * factor > inst:
            below -= CENT
        while (below + CENT) * factor <= inst:
            below += CENT
        disposable = below + CENT if above else below
        income = disposable + costs
        if 0 <= income <= 50000:
            break
    applicant["monthly_income"] = income
    exact = (below * factor == inst) and not above
    tag = "afford:eq" if exact else "afford:+0.01" if above else "afford:-0.01"
    return applicant, product, bureau, [tag, f"afford:{stage}"]


def boundary_cases(rng, n):
    out = []  # (applicant, product, bureau, tags)
    for age in AGE_EDGES:
        for _ in range(2):
            out.append((rand_applicant(rng, age=age), rand_product(rng), rand_bureau(rng), [f"age:{age}"]))
    for existing, edges in itertools.chain(PRE_EDGES.items(), POST_EDGES.items()):
        for edge in edges:
            for s in near(edge):
                out.append((applicant_with_score(rng, s, existing), rand_product(rng), rand_bureau(rng), [f"score:{s}"]))
    for existing, slo, shi, lo, hi in CREDIT_TABLES:
        scores = [s for s in REACHABLE if slo <= s <= shi]
        for edge in (lo, hi):
            for cs in (edge - 1, edge, edge + 1):
                a = applicant_with_score(rng, rng.choice(scores), existing)
                out.append((a, rand_product(rng), rand_bureau(rng, credit_score=cs), [f"credit:{cs}"]))
    k = 0
    while len(out) < n:
        out.append(afford_case(rng, k))
        k += 1
    return out[:n]


def generate(seed, n):
    rng = random.Random(seed)
    nb = round(n * 0.6)
    cases = boundary_cases(rng, nb)
    cases += [(rand_applicant(rng), rand_product(rng), rand_bureau(rng), ["random"]) for _ in range(n - nb)]
    rng.shuffle(cases)
    return [{"id": i + 1, "applicant": a, "product": p, "bureau": b, "tags": t} for i, (a, p, b, t) in enumerate(cases)]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--n", type=int, default=200)
    ap.add_argument("--out", default=str(HERE / "0087/heldout/inputs.jsonl"))
    args = ap.parse_args()
    cases = generate(args.seed, args.n)
    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text("".join(dumps(c) + "\n" for c in cases))
    counts = {}
    for c in cases:
        for t in c["tags"]:
            kind = t.split(":")[0]
            counts[kind] = counts.get(kind, 0) + 1
    print(f"wrote {len(cases)} cases to {out}; tag kinds: {counts}")


if __name__ == "__main__":
    main()
