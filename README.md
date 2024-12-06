# PAM-FaceAuthentication

## Overview

This python app is a facial recognition authentication module for Linux systems. It integrates with Pluggable Authentication Modules (PAM) to provide secure and convenient user authentication using facial recognition technology. ( Under development )

## Requirements

- Linux operating system
- Python 3.x
- OpenCV
- dlib
- PAM development libraries

## Installation

1. **Clone the repository:**

    ```bash
    git clone https://github.com/Dax2405/PAM-FaceAuthentication.git
    cd PAM-FaceAuthentication
    ```

2. **Install dependencies:**

    On debain based distros:
    
    ```bash
    sudo apt install cmake
    sudo apt install python-dev|python-devel
    sudo apt-get update
    sudo apt-get install -y python3 python3-pip libpam0g-dev
    pip3 install opencv-python dlib
    ```
    On Fedora:
    ```bash
    sudo dnf install cmake make-devel make gcc gcc-c++
    ```


## Troubleshooting

- Ensure your camera is working properly.
- Check the PAM configuration file for errors.
- Verify that all dependencies are installed correctly.

## Contributing

Contributions are welcome! Please fork the repository and submit a pull request.

## License

This project is licensed under the MIT License. See the [LICENSE](LICENSE) file for details.

## Contact

For any questions or issues, please open an issue on the [GitHub repository](https://github.com/yourusername/PAM-FaceAuthentication).
