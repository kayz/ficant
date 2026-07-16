import argparse
import json
import pathlib
import sys
import tomllib

try:
    import jsonschema
except ImportError as exc:
    raise SystemExit(f"jsonschema is required: {exc}")


def load_json(path: pathlib.Path):
    with path.open("r", encoding="utf-8-sig") as stream:
        return json.load(stream)


def validate_config(root: pathlib.Path) -> None:
    for name in ("profiles.toml", "environment-capabilities.toml"):
        with (root / name).open("rb") as stream:
            document = tomllib.load(stream)
        if document.get("schema_version") != 3:
            raise ValueError(f"{name} schema_version must be 3")
    for name in ("contract.schema.json", "result.schema.json", "fixed-income-wave1-result.schema.json"):
        schema = load_json(root / "schemas" / name)
        jsonschema.Draft202012Validator.check_schema(schema)


def validate_instance(schema_path: pathlib.Path, instance_path: pathlib.Path) -> None:
    schema = load_json(schema_path)
    jsonschema.Draft202012Validator.check_schema(schema)
    jsonschema.Draft202012Validator(schema).validate(load_json(instance_path))


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("action", choices=("config", "instance"))
    parser.add_argument("--root", type=pathlib.Path)
    parser.add_argument("--schema", type=pathlib.Path)
    parser.add_argument("--instance", type=pathlib.Path)
    args = parser.parse_args()
    try:
        if args.action == "config":
            validate_config(args.root)
        else:
            validate_instance(args.schema, args.instance)
    except Exception as exc:
        print(str(exc), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
