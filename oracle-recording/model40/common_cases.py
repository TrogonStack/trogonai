"""The common cases_sha256 check of mg37 (PLAN Updates 96 and 107), with the Ruling 42 item 4 exemption.

usage: python3 -I common_cases.py MARGINS37 HOST FIRST_BODY SECOND_BODY

FIRST_BODY and SECOND_BODY are the `body` texts of one host's first and second call legs. The two calls record one case
set, except for the sides margins37 gives a margin per call (`per_call`): each call places that side at its own margin,
so the case it names differs. The names left out of each call are read from margins37 itself, never listed here.
Prints both hashes over the case set without the call's own per-call names and exits 1 when they differ.
"""
import hashlib, json, sys


def exempt_names(margins, host, call):
    """The names of the cases `call` places at a per-call margin: pair id and margin, as the pair's case_at names them."""
    names = set()
    for pair, entry in margins[host].items():
        for side in entry["sides"].values():
            held = side.get("per_call", {}).get(call)
            if held is not None and "union_margin" in held:
                names.add(f"{pair}_{held['union_margin']}")
    return names


def common_sha256(body_text, skip):
    """sha256 of the entries minus `skip`, as r3.py's mg37 unit hashes its entries (json.dumps, sort_keys)."""
    entries = [entry for entry in json.loads(body_text)["cases"] if entry["name"] not in skip]
    return hashlib.sha256(json.dumps(entries, sort_keys=True).encode()).hexdigest(), [entry["name"] for entry in entries]


def main(margins_path, host, first_path, second_path):
    margins = json.load(open(margins_path, encoding="utf-8"))
    result = {}
    for call, path in (("first", first_path), ("second", second_path)):
        skip = exempt_names(margins, host, call)
        sha, names = common_sha256(open(path, encoding="utf-8").read(), skip)
        result[call] = {"sha256": sha, "cases": len(names), "exempt": sorted(skip)}
    print(json.dumps(result, indent=1))
    return 0 if result["first"]["sha256"] == result["second"]["sha256"] else 1


if __name__ == "__main__":
    sys.exit(main(*sys.argv[1:5]))
