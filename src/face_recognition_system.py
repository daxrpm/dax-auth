import cv2
import face_recognition
import time
import pickle
import os

NUM_FACES = 3
ENCODINGS_DIR = "data/encodings"
REGISTER_IMAGES_DIR = "data/register_images"
VIDEO_DEVICE = 0


def clear_directory(directory):
    for file in os.listdir(directory):
        file_path = os.path.join(directory, file)
        if os.path.isfile(file_path):
            os.unlink(file_path)


def register_face():
    # todo create the directories if they don't exist
    os.makedirs(ENCODINGS_DIR, exist_ok=True)
    os.makedirs(REGISTER_IMAGES_DIR, exist_ok=True)
    clear_directory(ENCODINGS_DIR)
    clear_directory(REGISTER_IMAGES_DIR)

    video_capture = cv2.VideoCapture(VIDEO_DEVICE)

    for i in range(NUM_FACES):
        time.sleep(1)

        result, image = video_capture.read()
        if result:
            cv2.imwrite(os.path.join(REGISTER_IMAGES_DIR,
                        f"register_face{i}.png"), image)

    video_capture.release()
    create_faces_encoding_file()


def create_faces_encoding_file():
    for i in range(NUM_FACES):
        image_path = os.path.join(REGISTER_IMAGES_DIR, f"register_face{i}.png")
        image_cv2 = cv2.imread(image_path)
        image_rgb = cv2.cvtColor(image_cv2, cv2.COLOR_BGR2RGB)
        # Detect the face using face_recognition
        try:
            encoding = face_recognition.face_encodings(image_rgb)[0]
            encoding_path = os.path.join(ENCODINGS_DIR, f"encoding{i}.pickle")
            with open(encoding_path, "wb") as file:
                pickle.dump(encoding, file)
        except IndexError:
            print(f"Face {i} not found")
            continue
        except Exception as e:
            print(f"An error occurred: {e}")
            continue


def read_faces_encoding_file():
    encodings = []
    for i in range(NUM_FACES):
        encoding_path = os.path.join(ENCODINGS_DIR, f"encoding{i}.pickle")
        with open(encoding_path, "rb") as file:
            encoding = pickle.load(file)
            encodings.append(encoding)
    return encodings


def verify_face():
    try:
        encodings = read_faces_encoding_file()
    except FileNotFoundError:
        print("No faces registered")
        return

    video_capture = cv2.VideoCapture(VIDEO_DEVICE)
    result, image = video_capture.read()
    if result:
        image_rgb = cv2.cvtColor(image, cv2.COLOR_BGR2RGB)
        image_encoding = face_recognition.face_encodings(image_rgb)[0]
        final_result = face_recognition.compare_faces(
            encodings, image_encoding)
        print(final_result)
    video_capture.release()


if __name__ == "__main__":
    verify_face()
