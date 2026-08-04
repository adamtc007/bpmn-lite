import unittest

from split_validation import validate_record_split


def record(example_id, family, group, label="op.test"):
    return {
        "example_id": example_id,
        "family_id": family,
        "label": label,
        "semantic_v3": {"split_group": group},
    }


class SplitValidationTests(unittest.TestCase):
    def test_family_leak_is_refused(self):
        records = [
            record("a", "family-a", "family-a"),
            record("b", "family-a", "family-a"),
            record("n1", "nota-train", "nota-train", "abstain.none_of_the_above"),
            record("n2", "nota-hold", "nota-hold", "abstain.none_of_the_above"),
        ]
        with self.assertRaisesRegex(ValueError, "family/group leakage"):
            validate_record_split(
                records,
                {"a": "train", "b": "test", "n1": "train", "n2": "test"},
            )

    def test_all_nota_in_training_is_refused(self):
        records = [
            record("n1", "nota-a", "nota-a", "abstain.none_of_the_above"),
            record("n2", "nota-b", "nota-b", "abstain.none_of_the_above"),
        ]
        with self.assertRaisesRegex(ValueError, "distinct train and holdout"):
            validate_record_split(records, {"n1": "train", "n2": "train"})


if __name__ == "__main__":
    unittest.main()
