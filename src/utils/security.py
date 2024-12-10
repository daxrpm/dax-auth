from cryptography.fernet import Fernet
from load_config import load_config


config = load_config()
SECRET_KEY_DIR = config["secret_key"]

key = Fernet.generate_key()
with open(f"{SECRET_KEY_DIR}/secret.key", "wb") as key_file:
    key_file.write(key)


def load_key():
    return open(f"{SECRET_KEY_DIR}/secret.key", "rb").read()


def encrypt_data(data):
    key = load_key()
    f = Fernet(key)
    return f.encrypt(data)


def decrypt_data(data):
    key = load_key()
    f = Fernet(key)
    return f.decrypt(data)
