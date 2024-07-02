import cv2
import face_recognition
import time
import pickle
import os

# Register a new face

num_faces = 3


def register_face():
    # remove past face data
    os.system("rm data/encodings/*")
    os.system("rm data/register_images/*")

    video_capture = cv2.VideoCapture(0)

    for i in range(num_faces):
        result, image = video_capture.read()

        if result:

            cv2.imwrite(f"data/register_images/register_face{i}.png", image)

        time.sleep(1)
    create_faces_encoding_file()


def create_faces_encoding_file():
    for i in range(num_faces):
        image_cv2 = cv2.imread(f"data/register_images/register_face{i}.png")
        image_rgb = cv2.cvtColor(image_cv2, cv2.COLOR_BGR2RGB)
        encoding = (face_recognition.face_encodings(image_rgb)[0])
        with open(f"data/encodings/encoding{i}.pickle", "wb") as file:
            pickle.dump(encoding, file)


def read_faces_encoding_file():
    encodings = []
    for i in range(num_faces):
        with open(f"data/encodings/encoding{i}.pickle", "rb") as file:
            encoding = pickle.load(file)
            encodings.append(encoding)
    return encodings


def verify_face():
    encodings = read_faces_encoding_file()
    video_capture = cv2.VideoCapture(0)
    result, image = video_capture.read()
    if result:
        image_encoding = face_recognition.face_encodings(image)[0]
        final_result = face_recognition.compare_faces(
            encodings, image_encoding)

        print(final_result)


verify_face()
