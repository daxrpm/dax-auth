import cv2
import face_recognition
import logging
from utils.face_operations import read_faces_encoding_file
from utils.load_config import load_config

# Configure logging
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
        return
    except Exception as e:
        logger.error(f"Error reading face encodings: {e}")
        return

    video_device = load_video_device()
    video_capture = cv2.VideoCapture(video_device)
    if not video_capture.isOpened():
        logger.error(f"Failed to open video device {video_device}")
        return

    try:
        result, image = video_capture.read()
        if not result:
            logger.error("Failed to capture image from video device")
            return

        image_rgb = cv2.cvtColor(image, cv2.COLOR_BGR2RGB)
        face_encodings = face_recognition.face_encodings(image_rgb)
        if not face_encodings:
            logger.warning("No faces detected in the image")
            return

        image_encoding = face_encodings[0]
        final_result = face_recognition.compare_faces(
            encodings, image_encoding)
        logger.info(f"Face verification result: {final_result}")
    except Exception as e:
        logger.error(f"Error during face verification: {e}")
    finally:
        video_capture.release()


if __name__ == "__main__":
    verify_face()
