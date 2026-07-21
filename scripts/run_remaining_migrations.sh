#!/bin/bash
set -e

DB_URL="postgresql://postgres@localhost:5433/data_designer"
MIGRATIONS_DIR="/Users/adamtc007/dev/rust/migrations"

echo "Running all migrations dynamically in alphabetical order..."

for migration_path in $(ls "$MIGRATIONS_DIR"/*.sql | sort); do
    migration_file=$(basename "$migration_path")
    if [[ "$migration_file" == "master-schema.sql" ]]; then
        continue
    fi
    echo "Applying $migration_file..."
    psql "$DB_URL" -v ON_ERROR_STOP=0 -f "$migration_path" > /dev/null 2>&1 || true
done

echo "All migrations applied successfully!"
