#include <security/pam_modules.h>
#include <stdio.h>
#include <errno.h>
#include <stdlib.h>
#include <string.h>
#include <syslog.h>
#include <unistd.h>
#include <sys/wait.h>

#define PYTHON_SCRIPT_PATH "/opt/daxauth/src/cli/verify.py"
#define PYTHON_COMMAND "/opt/daxauth/.venv/bin/python3"

PAM_EXTERN int pam_sm_authenticate(pam_handle_t *pamh, int flags __attribute__((unused)), int argc __attribute__((unused)), const char **argv __attribute__((unused)))
{
    const char *user;
    int retval = pam_get_user(pamh, &user, "Username: ");
    if (retval != PAM_SUCCESS)
    {
        return retval;
    }

    char command[256];
    snprintf(command, sizeof(command), "%s %s", PYTHON_COMMAND, PYTHON_SCRIPT_PATH);

    int status = system(command);
    if (status == -1)
    {
        syslog(LOG_ERR, "Failed to execute verify.py: %s", strerror(errno));
        return PAM_AUTH_ERR;
    }

    if (WIFEXITED(status) && WEXITSTATUS(status) == 0)
    {
        syslog(LOG_INFO, "Face verification successful for user %s", user);
        return PAM_SUCCESS;
    }
    else
    {
        syslog(LOG_ERR, "Face verification failed for user %s", user);
        return PAM_AUTH_ERR;
    }
}

PAM_EXTERN int pam_sm_setcred(pam_handle_t *pamh __attribute__((unused)), int flags __attribute__((unused)), int argc __attribute__((unused)), const char **argv __attribute__((unused)))
{
    return PAM_IGNORE;
}

PAM_EXTERN int pam_sm_acct_mgmt(pam_handle_t *pamh __attribute__((unused)), int flags __attribute__((unused)), int argc __attribute__((unused)), const char **argv __attribute__((unused)))
{
    return PAM_IGNORE;
}

PAM_EXTERN int pam_sm_open_session(pam_handle_t *pamh __attribute__((unused)), int flags __attribute__((unused)), int argc __attribute__((unused)), const char **argv __attribute__((unused)))
{
    return PAM_SUCCESS;
}

PAM_EXTERN int pam_sm_close_session(pam_handle_t *pamh __attribute__((unused)), int flags __attribute__((unused)), int argc __attribute__((unused)), const char **argv __attribute__((unused)))
{
    return PAM_IGNORE;
}

PAM_EXTERN int pam_sm_chauthtok(pam_handle_t *pamh __attribute__((unused)), int flags __attribute__((unused)), int argc __attribute__((unused)), const char **argv __attribute__((unused)))
{
    return PAM_IGNORE;
}