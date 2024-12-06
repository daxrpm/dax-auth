#!.venv/bin/python
import sys
import os

sys.path.append(os.path.join(os.path.dirname(__file__), '..'))

import argparse
from cli.add import register_face
from cli.clear import clear_directory
from cli.verify import verify_face
from utils.load_config import load_config
def load_configuration():
    """Load the configuration file."""
    try:
        return load_config()
    except Exception as e:
        print(f"Error loading configuration: {e}")
        sys.exit(1)


config = load_configuration()
ENCODINGS_DIR = config["encodings_dir"]
REGISTER_IMAGES_DIR = config["register_images_dir"]


def clear_data():
    """Clear face encodings and register images."""
    clear_directory(ENCODINGS_DIR)
    clear_directory(REGISTER_IMAGES_DIR)


def main():
    """Main function to handle command-line arguments and execute commands."""
    parser = argparse.ArgumentParser(description="Face authentication system")
    subparsers = parser.add_subparsers(
        dest="command", help="Available commands", required=True)

    parser_add = subparsers.add_parser("add", help="Register new face")
    parser_add.set_defaults(func=register_face)

    parser_clear = subparsers.add_parser(
        "clear", help="Clear face encodings and register images")
    parser_clear.set_defaults(func=clear_data)

    parser_verify = subparsers.add_parser(
        "verify", help="Verify face with registered faces")
    parser_verify.set_defaults(func=verify_face)

    args = parser.parse_args()

    if args.command in ["add", "clear"] and os.geteuid() != 0:
        print("This command must be run with superuser privileges (sudo).")
        sys.exit(1)

    args.func()


if __name__ == "__main__":
    main()
