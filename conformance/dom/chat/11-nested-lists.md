Here's the migration checklist:

- **Phase 1: prepare**
  - Freeze schema changes on `main`
  - Snapshot the database
    1. `pg_dump -Fc app > app.dump`
    2. Upload to `s3://backups/app/`
  - Notify #ops

- **Phase 2: migrate**

  Run the migration in a transaction:

  ```sql
  BEGIN;
  ALTER TABLE users ADD COLUMN plan text NOT NULL DEFAULT 'free';
  COMMIT;
  ```

- **Phase 3: verify**
  1. Smoke test the login flow
  2. Check error rates on the dashboard
     > If 5xx > 1% for 5 minutes, roll back.

3. Final step, numbered from three:
4. Celebrate 🎉

* [ ] Write the postmortem
* [x] Close the ticket
