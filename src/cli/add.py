import os
import time
import cv2
import logging
from utils.file_operations import clear_directory
from utils.face_operations import create_faces_encoding_file
from utils.load_config import load_config

logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)


def register_face():
    config = load_config()
    num_faces = config.get("num_faces", 5)
    encodings_dir = config.get("encodings_dir", "./encodings")
    register_images_dir = config.get(
        "register_images_dir", "./register_images")
    video_device = config.get("video_device", 0)

    try:
        os.makedirs(encodings_dir, exist_ok=True)
        os.makedirs(register_images_dir, exist_ok=True)
        clear_directory(encodings_dir)
        clear_directory(register_images_dir)

        video_capture = cv2.VideoCapture(video_device)
        if not video_capture.isOpened():
            raise RuntimeError("Could not open video device")

        for i in range(num_faces):
            time.sleep(1)
            result, image = video_capture.read()
            if not result:
                raise RuntimeError(f"Failed to capture image {i}")
            image_path = os.path.join(register_images_dir, f"face{i}.png")
            cv2.imwrite(image_path, image)

        video_capture.release()
        # TODO 1: Implement encription of face encodings
        create_faces_encoding_file()
    except Exception as e:
        logger.error(f"An error occurred during face registration: {e}")
    finally:
        if video_capture.isOpened():
            video_capture.release()


if __name__ == "__main__":
    register_face()
