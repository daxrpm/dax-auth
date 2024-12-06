CC = gcc
CFLAGS = -fPIC -Wall -Wextra
LDFLAGS = -shared

TARGET = pam_face_auth.so

all: $(TARGET)

$(TARGET): pam_face_auth.o
	$(CC) $(LDFLAGS) -o $@ $^

pam_face_auth.o: pam_face_auth.c
	$(CC) $(CFLAGS) -c $<

clean:
	rm -f *.o $(TARGET)