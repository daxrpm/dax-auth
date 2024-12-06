import cv2
import face_recognition
import os
import pickle
import logging
from .load_config import load_config

logging.basicConfig(level=logging.INFO,
                    format='%(asctime)s - %(levelname)s - %(message)s')
logger = logging.getLogger(__name__)

config = load_config()

NUM_FACES = config["num_faces"]
REGISTER_IMAGES_DIR = config["register_images_dir"]
ENCODINGS_DIR = config["encodings_dir"]


def create_faces_encoding_file():
    """
    Create encoding files for registered faces.
    """
    for face_index in range(NUM_FACES):
        image_path = os.path.join(REGISTER_IMAGES_DIR, f"face{face_index}.png")
        try:
            image_cv2 = cv2.imread(image_path)
            if image_cv2 is None:
                logger.warning(
                    f"Image {image_path} not found or cannot be read.")
                continue

            image_rgb = cv2.cvtColor(image_cv2, cv2.COLOR_BGR2RGB)
            encodings = face_recognition.face_encodings(image_rgb)
            if not encodings:
                logger.warning(f"No face found in image {image_path}.")
                continue

            encoding = encodings[0]
            encoding_path = os.path.join(
                ENCODINGS_DIR, f"encoding{face_index}.pickle")
            with open(encoding_path, "wb") as encoding_file:
                pickle.dump(encoding, encoding_file)
            logger.info(f"Encoding for face {face_index} saved successfully.")
        except Exception as e:
            logger.error(f"An error occurred while processing {
                         image_path}: {e}")


def read_faces_encoding_file():
    """
    Read encoding files for registered faces.
    """
    encodings = []
    for face_index in range(NUM_FACES):
        encoding_path = os.path.join(
            ENCODINGS_DIR, f"encoding{face_index}.pickle")
        try:
            with open(encoding_path, "rb") as encoding_file:
                encoding = pickle.load(encoding_file)
                encodings.append(encoding)
            logger.info(f"Encoding for face {face_index} loaded successfully.")
        except FileNotFoundError:
            logger.warning(f"Encoding file {encoding_path} not found.")
        except Exception as e:
            logger.error(f"An error occurred while reading {
                         encoding_path}: {e}")
    return encodings
