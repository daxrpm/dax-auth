import os
import logging

logging.basicConfig(level=logging.INFO)


def clear_directory(directory):
    if not os.path.isdir(directory):
        logging.error(f"The provided path '{directory}' is not a directory.")
        return

    try:
        for file in os.listdir(directory):
            file_path = os.path.join(directory, file)
            if os.path.isfile(file_path):
                os.unlink(file_path)
                logging.info(f"Deleted file: {file_path}")
    except Exception as e:
        logging.error(f"An error occurred while clearing the directory: {e}")
