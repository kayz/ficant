import hashlib
import json
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
MATRIX = Path(__file__).with_name("acceptance-matrix.json")


def main() -> None:
    payload = json.loads(MATRIX.read_text(encoding="utf-8"))
    assert payload["schema"] == "ficant.quality.iteration-3.acceptance-matrix.v1"
    expected_ids = [f"Q-{number:03d}" for number in range(1, 37)]
    actual_ids = [entry["id"] for entry in payload["acceptance"]]
    assert actual_ids == expected_ids, "acceptance IDs must be ordered and exactly Q-001..Q-036"
    assert len(set(actual_ids)) == 36

    expected_payload = json.loads(
        (ROOT / "tests/golden-cases/china-rates/expected/cgb-reference-v1-expected.json")
        .read_text(encoding="utf-8")
    )
    assert expected_payload["acceptance_ids"] == expected_ids[:23]
    mapped_cases = {
        entry["id"]: entry.get("case")
        for entry in payload["acceptance"][:12]
    }
    for acceptance_id, mapping in expected_payload["acceptance_mapping"].items():
        if mapping["cases"]:
            assert mapped_cases[acceptance_id] == mapping["cases"][0]

    for relative, expected_hash in payload["frozen_assets"].items():
        path = ROOT / relative
        actual_hash = hashlib.sha256(path.read_bytes()).hexdigest()
        assert actual_hash == expected_hash, f"frozen asset drift: {relative}"

    for entry in payload["acceptance"]:
        assert entry["category"]
        assert entry["automation"], f"{entry['id']} has no executable automation"
        for automation in entry["automation"]:
            source = ROOT / automation["source"]
            assert source.is_file(), f"missing source for {entry['id']}: {source}"
            assert automation["command"].strip(), f"empty command for {entry['id']}"
            selector = automation.get("selector")
            if selector:
                assert selector in source.read_text(encoding="utf-8"), (
                    f"missing selector for {entry['id']}: {selector}"
                )

    print("Q-001..Q-036 acceptance matrix: PASS (36 mapped, 0 missing, frozen assets unchanged)")


if __name__ == "__main__":
    main()
