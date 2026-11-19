#!/usr/bin/env python3
"""
Rotates password for Postgres role `aion_agent_conversation_store_app` in a single atomic process.
The secret is generated in memory, passed to PostgreSQL via stdin (no CLI args),
and written directly to .env.app with mode 0600.
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

    # 1. Generate new cryptographically secure secret
    new_password = secrets.token_hex(24)

    # 2. Update Postgres role via docker exec psql stdin
    alter_sql = f"ALTER ROLE aion_agent_conversation_store_app WITH PASSWORD '{new_password}';\n"
    proc = subprocess.Popen(
        ["docker", "exec", "-i", "aion_master_postgres_team", "psql", "-U", "root", "-d", "aion_agent_conversation_store"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True
    )
    stdout, stderr = proc.communicate(input=alter_sql)
    if proc.returncode != 0:
        sys.stderr.write(f"ERROR: Failed to alter role password in database: {stderr.strip()}\n")
        sys.exit(1)

    # 3. Write .env.app atomically with mode 0600 (both new and fallback legacy var)
    escaped_pw = urllib.parse.quote(new_password)
    db_url = f"postgres://aion_agent_conversation_store_app:{escaped_pw}@aion_master_postgres_team:5432/aion_agent_conversation_store"
    env_content = (
        f"AION_AGENT_CONVERSATION_STORE_DATABASE_URL={db_url}\n"
        f"AION_SESSION_STORE_DATABASE_URL={db_url}\n"
    )

    fd = os.open(env_file, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as f:
        f.write(env_content)

    print(f"[OK] Successfully rotated aion_agent_conversation_store_app password.")
    print(f"[OK] Updated {env_file} (mode 0600).")

if __name__ == "__main__":
    main()
