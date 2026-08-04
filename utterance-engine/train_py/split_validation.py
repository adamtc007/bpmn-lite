"""Fail-closed semantic-family and paired-context split validation."""


def validate_record_split(records, split):
    family_splits = {}
    pair_splits = {}
    nota_splits = set()
    for record in records:
        assigned = split[record["example_id"]]
        family_splits.setdefault(record["family_id"], set()).add(assigned)
        group = record["semantic_v3"]["split_group"]
        pair_splits.setdefault(group, set()).add(assigned)
        if record["label"] == "abstain.none_of_the_above":
            nota_splits.add(assigned)
    leaked_families = sorted(key for key, values in family_splits.items() if len(values) != 1)
    leaked_groups = sorted(key for key, values in pair_splits.items() if len(values) != 1)
    if leaked_families or leaked_groups:
        raise ValueError(
            f"family/group leakage: families={leaked_families}, groups={leaked_groups}"
        )
    if len(nota_splits) < 2:
        raise ValueError("NOTA requires distinct train and holdout families")
