CREATE OR REPLACE FUNCTION set_updated_at()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = now();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_instances_updated_at
    BEFORE UPDATE ON process_instances
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();
