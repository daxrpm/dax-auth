import os
import logging

logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)


def clear_directory(directory):
    """
    Clear the contents of a directory used as delete registered face.
    """
    if not os.path.isdir(directory):
        logger.error(f"The provided path '{directory}' is not a directory.")
        return

    try:
        for file in os.listdir(directory):
            file_path = os.path.join(directory, file)
            if os.path.isfile(file_path):
                os.unlink(file_path)
                logger.info(f"Deleted file: {file_path}")
    except Exception as e:
        logger.error(f"An error occurred while clearing the directory: {e}")
