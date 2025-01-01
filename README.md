# DaxAuth - PAM-FaceAuthentication

DaxAuth is a project that provides facial authentication for Linux systems using PAM (Pluggable Authentication Modules). This project is currently under development and may have some security risks, such as the potential to unlock with photos, also actually tested for some months it seems to have problems working in dark spaces

DaxAuth can be integrated in any system that uses PAM as authentication

## Instalation

### Automatic instalation

**Automatic instalation for:** Debian-based systems, Red Hat-based systems and Arch-based systems
### Clone the repo
```sh
git clone https://github.com/Dax2405/dax-auth.git
```
### Run the instalation script
```sh
cd dax-auth && ./install.sh
```
 The installation has been tested on Fedora 40-41, Debian, Ubuntu 24.04 - 24.10, and Arch Linux.

### Manual instalation

#### Dependencies Instalation

##### For Debian-based systems (using `apt`)

```sh
sudo apt-get install -y cmake make gcc g++ python3 python3-dev python3-pip python3-venv libpam0g-dev
```

##### For Red Hat-based systems (using `dnf`)

```sh
sudo dnf install -y cmake make gcc gcc-c++ python3 python3-devel python3-pip pam-devel
```

##### For Arch-based systems (using `pacman`)

```sh
sudo pacman -S --noconfirm cmake make gcc python python-pip python-virtualenv pam
```


#### Create necessary directories

```sh
sudo mkdir -p /opt/daxauth
sudo mkdir -p /var/lib/daxauth/data
```

#### Create and activate a Python virtual environment

```sh
sudo python3 -m venv /opt/daxauth/.venv
source /opt/daxauth/.venv/bin/activate
```

#### Install required Python packages

```sh
sudo /opt/daxauth/.venv/bin/pip install -r requirements.txt
```

#### Copy source and configuration files

```sh
sudo cp -r src /opt/daxauth
sudo cp -r config /opt/daxauth
```

#### Copy the main script to `/usr/local/bin` and make it executable

```sh
sudo cp src/daxauth /usr/local/bin/daxauth
sudo chmod +x /usr/local/bin/daxauth
```

#### Compile the C code for the PAM module

```sh
cd /opt/daxauth/src/pam
sudo make
```

#### Copy the compiled PAM module to the appropriate directory

The directory may change based on your distro

```sh
sudo cp pam_face_auth.so /lib/security
```

#### Backup the existing sudo PAM configuration file

```sh
sudo cp /etc/pam.d/sudo /etc/pam.d/sudo.bak
```

#### Modify the sudo PAM configuration to include the new module in sudo auth

You can add it too all the modules you want to work with

```sh
sudo grep -qF "auth sufficient pam_face_auth.so" /etc/pam.d/sudo || sudo sed -i '1a auth sufficient pam_face_auth.so' /etc/pam.d/sudo
```

## Usage CLI

### Commands

#### Register a New Face
To register a new face, use the `add` command. This command requires superuser privileges.
```sh
sudo daxauth add
```
This will prompt you to provide the necessary images for face registration.

#### Clear Face Encodings and Register Images
To clear all face encodings and registered images, use the `clear` command. This command also requires superuser privileges.
```sh
sudo daxauth clear
```
This will delete all stored face encodings and registered images.

#### Verify a Face
To verify a face against the registered faces, use the `verify` command.
```sh
daxauth verify
```
This will compare the provided face image with the registered faces and return the verification result.




## Contributing
If you would like to contribute to this project, please fork the repository and submit a pull request.

## License
This project is licensed under the GNU General Public License v3.0. See the LICENSE file for more details.

## Contact
For any questions or issues, you can contact me to this mail:
 dax@dax-ec.ru
