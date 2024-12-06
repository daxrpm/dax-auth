#! /home/dax/Escritorio/repos/PAM-FaceAuthentication/.venv/bin/python
import sys
import os

sys.path.append(os.path.join(os.path.dirname(__file__), '..', '..', 'src'))

import cv2
import face_recognition
import logging
from utils.face_operations import read_faces_encoding_file
from utils.load_config import load_config

logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)


def load_video_device() -> int:
    config = load_config()
    video_device = config.get("video_device")
    if video_device is None:
        logger.error("Video device not specified in the configuration.")
        raise ValueError("Video device not specified in the configuration.")
    return video_device


def verify_face() -> None:
    try:
        encodings = read_faces_encoding_file()
    except FileNotFoundError:
        logger.error("No faces registered")
        sys.exit(1)
    except Exception as e:
        logger.error(f"Error reading face encodings: {e}")
        sys.exit(1)

    video_device = load_video_device()
    video_capture = cv2.VideoCapture(video_device)
    if not video_capture.isOpened():
        logger.error(f"Failed to open video device {video_device}")
        sys.exit(1)

    try:
        result, image = video_capture.read()
        if not result:
            logger.error("Failed to capture image from video device")
            sys.exit(1)

        image_rgb = cv2.cvtColor(image, cv2.COLOR_BGR2RGB)
        face_encodings = face_recognition.face_encodings(image_rgb)
        if not face_encodings:
            logger.warning("No faces detected in the image")
            sys.exit(1)

        image_encoding = face_encodings[0]
        final_result = face_recognition.compare_faces(
            encodings, image_encoding)
        logger.info(f"Face verification result: {final_result}")

        true_count = final_result.count(True)
        false_count = final_result.count(False)

        #logger.info(f"True count: {true_count}, False count: {false_count}")

        threshold = len(encodings) // 2
        if true_count > threshold:
            logger.info("Face verification successful")
            sys.exit(0)
        else:
            logger.info("Face verification failed")
            sys.exit(1)
    except Exception as e:
        logger.error(f"Error during face verification: {e}")
        sys.exit(1)
    finally:
        video_capture.release()


if __name__ == "__main__":
    verify_face()
