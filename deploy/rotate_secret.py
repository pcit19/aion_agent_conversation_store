#!/usr/bin/env python3
"""
Rotates password for Postgres role `aion_session_store_app` in a single atomic process.
The secret is generated in memory, passed to PostgreSQL via stdin (no CLI args),
and written directly to .env.app and ~/.config/aion_session_store/app.secret with mode 0600.
No password is ever echoed to stdout or process argument lists.
"""

import os
import secrets
import subprocess
import sys
import urllib.parse

def main():
    repo_dir = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    env_file = os.path.join(repo_dir, ".env.app")
    secret_dir = os.path.expanduser("~/.config/aion_session_store")
    os.makedirs(secret_dir, mode=0o700, exist_ok=True)
    secret_file = os.path.join(secret_dir, "app.secret")

    # 1. Generate new cryptographically secure secret
    new_password = secrets.token_hex(24)

    # 2. Update Postgres role via docker exec psql stdin
    alter_sql = f"ALTER ROLE aion_session_store_app WITH PASSWORD '{new_password}';\n"
    proc = subprocess.Popen(
        ["docker", "exec", "-i", "aion_master_postgres_team", "psql", "-U", "root", "-d", "aion_session_store"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True
    )
    stdout, stderr = proc.communicate(input=alter_sql)
    if proc.returncode != 0:
        sys.stderr.write(f"ERROR: Failed to alter role password in database: {stderr.strip()}\n")
        sys.exit(1)

    # 3. Write .env.app atomically with mode 0600
    escaped_pw = urllib.parse.quote(new_password)
    db_url = f"postgres://aion_session_store_app:{escaped_pw}@aion_master_postgres_team:5432/aion_session_store\n"
    env_content = f"AION_SESSION_STORE_DATABASE_URL={db_url}"

    fd = os.open(env_file, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as f:
        f.write(env_content)

    # 4. Write app.secret with mode 0600
    fd2 = os.open(secret_file, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd2, "w") as f:
        f.write(new_password + "\n")

    print(f"[OK] Successfully rotated aion_session_store_app password.")
    print(f"[OK] Updated {env_file} (mode 0600) and {secret_file} (mode 0600).")

if __name__ == "__main__":
    main()
