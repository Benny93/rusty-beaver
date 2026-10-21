#!/usr/bin/env python3
"""End-to-end check: start the app (in the background, on throwaway files) and drive every
feature area over MCP, asserting on what comes back. Exit code 0 = all passed.

usage: smoke.py [binary]   (default target/debug/rusty-beaver)
"""
import json, os, sqlite3, subprocess, sys, tempfile, time

sys.path.insert(0, os.path.dirname(__file__))
import mcp  # noqa: E402

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DB = "Smoke"
failures = []


def call(tool, args=None, ok=True):
    mcp.session = None
    mcp.rpc("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "smoke", "version": "1"}})
    mcp.rpc("notifications/initialized", notify=True)
    res = mcp.rpc("tools/call", {"name": tool, "arguments": args or {}})
    text = res["content"][0].get("text", "")
    if res.get("isError") == ok:
        raise AssertionError(f"{tool} {'failed' if ok else 'should have failed'}: {text[:300]}")
    try:
        return json.loads(text)
    except ValueError:
        return text


def check(name, fn):
    try:
        fn()
        print(f"  ok    {name}")
    except Exception as e:  # noqa: BLE001 - report every failure, keep going
        failures.append(name)
        print(f"  FAIL  {name}: {e}")


def main():
    binary = sys.argv[1] if len(sys.argv) > 1 else os.path.join(ROOT, "target", "debug", "rusty-beaver")
    tmp = tempfile.mkdtemp()
    db = os.path.join(tmp, "smoke.db")
    con = sqlite3.connect(db)
    con.executescript(open(os.path.join(ROOT, "src", "demo.sql")).read())
    con.close()
    cfg = os.path.join(tmp, "connections.json")
    json.dump([{"name": DB, "driver": "sqlite", "database": db}], open(cfg, "w"))
    # the last session: two table tabs, the first one active
    json.dump({"scripts": [{"connection": DB, "table": "lodges"}, {"connection": DB, "table": "beavers"}]},
              open(os.path.join(tmp, "s.json"), "w"))
    port = os.environ.get("SMOKE_PORT", "7880")
    mcp.URL = f"http://127.0.0.1:{port}/mcp"
    env = dict(os.environ, RUSTY_BEAVER_CONFIG=cfg, RUSTY_BEAVER_MCP_PORT=port, RUSTY_BEAVER_BACKGROUND="1", RUSTY_BEAVER_KEYCHAIN="off",
               RUSTY_BEAVER_HISTORY=os.path.join(tmp, "h.jsonl"), RUSTY_BEAVER_SESSION=os.path.join(tmp, "s.json"))
    app = subprocess.Popen([binary], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        for _ in range(100):
            try:
                call("get_state")
                break
            except Exception:  # noqa: BLE001 - not up yet
                time.sleep(0.1)
        q = lambda sql: sqlite3.connect(db).execute(sql).fetchall()  # noqa: E731

        def restored_session():
            for _ in range(30):  # the active restored tab loads by itself
                r = call("get_results")
                if r["row_count"]:
                    break
                time.sleep(0.1)
            assert r["row_count"] == 4, r
            tabs = call("get_state")["tabs"]
            assert [t["active"] for t in tabs] == [True, False], tabs
            # a restored tab opened from the navigator does not load again when selected
            call("open_table", {"connection": DB, "table": "beavers"})
            call("edit_cell", {"row": 0, "column": "name", "value": "Kept"})
            call("select_tab", {"index": 0})
            call("select_tab", {"index": 1})
            assert "pending" not in call("get_state")["status"], call("get_state")["status"]
            call("revert_changes")
            call("close_tab", {"index": 1})
            call("close_tab", {"index": 0})
            # explain with nothing to explain leaves no empty editor behind
            call("explain", {}, ok=False)
            assert call("get_state")["tabs"] == [], call("get_state")["tabs"]

        def navigator():
            tables = [t["name"] for t in call("connect", {"connection": DB})["tables"]]
            assert tables == ["beavers", "leaderboard", "lodges", "missions"], tables
            cols = call("describe_table", {"connection": DB, "table": "missions"})["rows"]
            assert ["leader_id", "INTEGER", "YES", "FK", "beavers.id"] in cols, cols
            assert "no table" in call("describe_table", {"connection": DB, "table": "usres"}, ok=False)
            assert "usres" not in json.dumps(call("get_state")["selected"])  # not selected in the navigator
            before = len(call("get_state")["tabs"])
            call("open_table", {"connection": DB, "table": "usres"}, ok=False)
            assert len(call("get_state")["tabs"]) == before  # no tab for a table that is not there
            # nor for a connection that does not connect
            call("add_connection", {"name": "Broken", "driver": "sqlite", "database": "/nonexistent/dir/x.db"})
            call("open_table", {"connection": "Broken", "table": "t"}, ok=False)
            assert len(call("get_state")["tabs"]) == before, call("get_state")["tabs"]
            call("delete_connection", {"connection": "Broken"})
            # host, user and database are trimmed as the dialog does (a password is not)
            call("add_connection", {"name": "Spaced", "driver": "postgres", "host": " db.example.com ", "database": "app ", "user": " u"})
            sp = [c for c in call("get_state")["connections"] if c["name"] == "Spaced"][0]
            assert (sp["host"], sp["database"]) == ("db.example.com", "app"), sp
            call("delete_connection", {"connection": "Spaced"})
            # a name that differs only by spaces is the same name
            assert "exists" in call("add_connection", {"name": DB + " ", "driver": "sqlite", "database": db}, ok=False)
            # testing settings creates no database file
            typo = os.path.join(tmp, "typo.db")
            call("test_connection", {"name": "t", "driver": "sqlite", "database": typo}, ok=False)
            assert not os.path.exists(typo)
            # completion on a connection not connected yet still knows its tables
            call("add_connection", {"name": "Fresh", "driver": "sqlite", "database": db})
            labels = [c["label"] for c in call("sql_completions", {"connection": "Fresh", "sql": "SELECT * FROM bea"})]
            assert "beavers" in labels, labels
            call("delete_connection", {"connection": "Fresh"})
            # an id taken by another connection (its keychain entry) is not reused
            taken = json.load(open(cfg))[0]["id"]
            call("add_connection", {"name": "Dup Id", "driver": "sqlite", "database": db, "id": taken})
            ids = [c["id"] for c in json.load(open(cfg))]
            assert len(set(ids)) == len(ids), ids
            call("delete_connection", {"connection": "Dup Id"})
            assert "no table" in call("generate_sql", {"connection": DB, "table": "usres", "kind": "select"}, ok=False)

        def sql_editor():
            r = call("run_sql", {"connection": DB, "sql": "SELECT name FROM beavers ORDER BY logs_chewed DESC"})
            assert r["rows"][0] == ["Marco"] and r["row_count"] == 8
            r = call("run_sql", {"sql": "SELECT 1 AS a; SELECT 2 AS b;"})
            assert len(r["results"]) == 2 and r["rows"] == [["2"]]
            assert call("get_results", {"index": 0})["rows"] == [["1"]]
            call("search_results", {"text": "2"})  # the rows as the grid shows them
            assert call("get_results", {"index": 0})["rows"] == [], "search hides 1"
            call("search_results", {"text": ""})
            assert "no such column" in call("run_sql", {"sql": "SELECT nope"}, ok=False)
            call("run_sql", {"sql": "SELECT NULL AS n, 'NULL' AS t"})  # SQL NULL vs the text
            assert call("show_value", {"row": 0, "column": "n"})["value"] is None
            assert call("show_value", {"row": 0, "column": "t"})["value"] == "NULL"
            assert call("record_view", {"on": True, "row": 0})["record"] == {"n": None, "t": "NULL"}
            call("record_view", {"on": False})
            call("new_sql_editor", {"connection": DB})
            assert "empty" in call("execute_editor", {}, ok=False)  # refused, not "started"
            assert "absolute" in call("save_script", {"path": "relative.sql"}, ok=False)
            # a script saved under its own name keeps its number taken
            title = [t for t in call("get_state")["tabs"] if t["active"]][0]["title"]
            name = title.split(" ")[0]
            call("save_script", {"path": os.path.join(tmp, name + ".sql")})
            call("new_sql_editor", {"connection": DB})
            again = [t for t in call("get_state")["tabs"] if t["active"]][0]["title"]
            assert not again.startswith(name + " "), (title, again)

        def params_format_explain():
            r = call("run_sql", {"sql": "SELECT count(*) FROM beavers WHERE rank = :rank", "params": {"rank": "'Private'"}})
            assert r["rows"] == [["2"]], r
            # numbers and null as JSON values, not only strings
            assert call("run_sql", {"sql": "SELECT :n + 1", "params": {"n": 41}})["rows"] == [["42"]]
            assert call("format_sql", {"sql": "select a from t where b = 1"})["sql"].startswith("SELECT\n    a\nFROM t")
            assert call("explain", {"sql": "SELECT * FROM beavers WHERE id = 1"})["row_count"] >= 1
            assert "one statement" in call("explain", {"sql": "SELECT 1; DELETE FROM missions"}, ok=False)
            labels = [c["label"] for c in call("sql_completions", {"connection": DB, "sql": "SELECT b. FROM beavers b", "offset": 9})]
            assert "logs_chewed" in labels, labels
            # a dropped table's columns go with it
            call("run_sql", {"connection": DB, "sql": "CREATE TABLE gone(zap_col INT)"})
            time.sleep(0.5)  # the catalog loads in the background
            done = lambda: [c["label"] for c in call("sql_completions", {"connection": DB, "sql": "SELECT zap", "offset": 10})]
            assert "zap_col" in done(), done()
            call("describe_table", {"connection": DB, "table": "gone"})  # selected in the navigator
            call("run_sql", {"connection": DB, "sql": "DROP TABLE gone"})
            call("refresh_connection", {"connection": DB})
            assert call("get_state")["selected"]["table"] is None  # not a table that is gone
            time.sleep(0.5)
            assert "zap_col" not in done(), done()

        def table_editing():
            assert call("open_table", {"connection": DB, "table": "lodges"})["row_count"] == 4
            call("edit_cell", {"row": 0, "column": "river", "value": "Yukon River"})
            call("add_row", {"values": {"name": "Smoke Lodge", "river": "Elbe"}})
            call("delete_row", {"row": 3})  # referenced by beavers: the foreign key refuses
            assert "FOREIGN KEY" in call("save_changes", ok=False)
            call("delete_row", {"row": 3})  # unmark it
            assert call("save_changes")["saved"]
            assert q("SELECT river FROM lodges WHERE id = 1") == [("Yukon River",)]
            assert q("SELECT count(*) FROM lodges") == [(5,)]
            assert call("set_filter", {"filter": "river = 'Elbe'"})["row_count"] == 1
            call("edit_cell", {"row": 0, "column": "river", "value": "Rhine"})
            assert "pending changes" in call("set_filter", {"filter": ""}, ok=False)  # edit kept
            active = [t["index"] for t in call("get_state")["tabs"] if t["active"]][0]
            assert "pending changes" in call("close_tab", {"index": active}, ok=False)
            call("revert_changes")
            call("explain")  # the plan opens in a script tab, not in the editable table grid
            active = [t for t in call("get_state")["tabs"] if t["active"]]
            assert active[0]["title"].startswith("Script-"), active
            call("open_table", {"connection": DB, "table": "lodges"})
            call("open_table", {"connection": DB, "table": "missions"})
            assert "beavers" in call("open_referenced_row", {"row": 0, "column": "leader_id"})["ok"]
            for _ in range(50):  # the referenced table loads in the background
                r = call("get_results")
                if r["row_count"] == 1:
                    break
                time.sleep(0.1)
            assert r["rows"][0][1] == "Marco", r
            call("open_table", {"connection": DB, "table": "lodges"})
            ddl = call("set_table_view", {"view": "ddl"})["ddl"]
            assert ddl.startswith("CREATE TABLE lodges"), ddl
            call("set_table_view", {"view": "data"})
            # a row marked for deletion is no row "not loaded yet"
            call("open_table", {"connection": DB, "table": "lodges"})
            call("delete_row", {"row": 0})
            out = call("export_csv", {"path": os.path.join(tmp, "del.csv")})["ok"]
            assert "Load more" not in out, out
            call("revert_changes")
            # a row added and deleted again is no change, also in the reply
            call("open_table", {"connection": DB, "table": "lodges"})
            call("add_row", {"values": {"name": "Gone", "river": None}})
            rows = call("get_results")["row_count"]
            c = call("delete_row", {"row": rows - 1})
            assert (c["pending_changes"], c["added_rows"], c["deleted_rows"]) == (0, 0, []), c
            call("revert_changes")
            # a duplicated row leaves out generated columns (they cannot be inserted)
            call("run_sql", {"connection": DB, "sql": "CREATE TABLE gen(id INTEGER PRIMARY KEY, a INT, b INT GENERATED ALWAYS AS (a * 2)); INSERT INTO gen(a) VALUES (3)"})
            call("open_table", {"connection": DB, "table": "gen"})
            call("duplicate_row", {"row": 0})
            assert call("save_changes")["saved"]
            assert q("SELECT a, b FROM gen ORDER BY id") == [(3, 6), (3, 6)]
            # table tabs have no search box: a search there could not be cleared by the user
            call("open_table", {"connection": DB, "table": "lodges"})
            assert "set_filter" in call("search_results", {"text": "x"}, ok=False)
            # a Load more that fails does not make the next one fetch two pages
            call("run_sql", {"connection": DB, "sql": "CREATE TABLE big AS WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 1000) SELECT i FROM n"})
            assert call("open_table", {"connection": DB, "table": "big"})["row_count"] == 200
            call("run_sql", {"connection": DB, "sql": "DROP TABLE big"})
            big_tab = [t["index"] for t in call("get_state")["tabs"] if t["title"] == "big"][0]
            call("select_tab", {"index": big_tab})
            call("load_more", {}, ok=False)
            call("run_sql", {"connection": DB, "sql": "CREATE TABLE big AS WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 1000) SELECT i FROM n"})
            call("select_tab", {"index": big_tab})
            more = call("load_more", {})["row_count"]
            assert more == 400, more

        def transactions_and_production():
            call("set_auto_commit", {"connection": DB, "on": False})
            call("run_sql", {"connection": DB, "sql": "DELETE FROM missions"})
            call("rollback", {"connection": DB})
            assert q("SELECT count(*) FROM missions") == [(6,)]
            # a COMMIT that SQLite refuses (deferred foreign key) leaves the transaction open
            call("run_sql", {"connection": DB, "sql": "PRAGMA foreign_keys = ON"})
            call("run_sql", {"connection": DB, "sql": "CREATE TABLE fk_p(id INTEGER PRIMARY KEY); CREATE TABLE fk_c(pid INTEGER REFERENCES fk_p(id) DEFERRABLE INITIALLY DEFERRED)"})
            call("commit", {"connection": DB})
            call("run_sql", {"connection": DB, "sql": "INSERT INTO fk_c VALUES (99)"})
            call("commit", {"connection": DB}, ok=False)
            assert [c for c in call("get_state")["connections"] if c["name"] == DB][0]["in_transaction"]
            call("rollback", {"connection": DB})
            call("run_sql", {"connection": DB, "sql": "DROP TABLE fk_c; DROP TABLE fk_p"})
            call("commit", {"connection": DB})
            # a write that fails is not counted in the open transaction
            call("run_sql", {"connection": DB, "sql": "INSERT INTO lodges(id, name, river) VALUES (900, 'Tx Count', 'Main')"})
            call("run_sql", {"connection": DB, "sql": "INSERT INTO lodges(id, name, river) VALUES (900, 'Tx Count', 'Main')"}, ok=False)
            conn = [c for c in call("get_state")["connections"] if c["name"] == DB][0]
            assert conn["statements_in_transaction"] == 1, conn
            call("rollback", {"connection": DB})
            # a session setting opens no transaction (SQLite ignores foreign_keys inside one)
            call("run_sql", {"connection": DB, "sql": "PRAGMA foreign_keys = ON"})
            assert not [c for c in call("get_state")["connections"] if c["name"] == DB][0]["in_transaction"]
            # marking the connection production keeps its open transaction
            call("run_sql", {"connection": DB, "sql": "INSERT INTO lodges(name, river) VALUES ('Prod Toggle', 'Main')"})
            call("edit_connection", {"connection": DB, "production": True})
            call("edit_connection", {"connection": DB, "production": False})
            # a change that reconnects would roll it back: refused while it is open
            assert "transaction" in call("edit_connection", {"connection": DB, "read_only": True}, ok=False)
            assert [c for c in call("get_state")["connections"] if c["name"] == DB][0]["in_transaction"]
            call("commit", {"connection": DB})
            assert q("SELECT count(*) FROM lodges WHERE name = 'Prod Toggle'") == [(1,)]
            # a script that fails before its COMMIT leaves the transaction it opened open
            call("run_sql", {"connection": DB, "sql": "UPDATE lodges SET name = name WHERE nope = 1; COMMIT"}, ok=False)
            assert [c for c in call("get_state")["connections"] if c["name"] == DB][0]["in_transaction"]
            call("rollback", {"connection": DB})
            # a COMMIT at the end of a script ends the transaction it opened
            call("run_sql", {"connection": DB, "sql": "INSERT INTO lodges(name, river) VALUES ('Tx Lodge', 'Main'); COMMIT"})
            assert not [c for c in call("get_state")["connections"] if c["name"] == DB][0]["in_transaction"]
            assert q("SELECT count(*) FROM lodges WHERE name = 'Tx Lodge'") == [(1,)]
            # SQLite ended the transaction itself (ON CONFLICT ROLLBACK): Rollback still ends it here
            call("run_sql", {"connection": DB, "sql": "CREATE TABLE u(id INTEGER PRIMARY KEY ON CONFLICT ROLLBACK); INSERT INTO u VALUES (1); COMMIT"})
            call("run_sql", {"connection": DB, "sql": "INSERT INTO u VALUES (1)"}, ok=False)
            call("rollback", {"connection": DB}, ok=False)  # SQLite: no transaction is active
            assert not [c for c in call("get_state")["connections"] if c["name"] == DB][0]["in_transaction"]
            # a BEGIN typed in manual mode opens the transaction the next write uses
            call("run_sql", {"connection": DB, "sql": "BEGIN"})
            call("run_sql", {"connection": DB, "sql": "INSERT INTO u VALUES (5)"})
            call("rollback", {"connection": DB})
            assert q("SELECT count(*) FROM u WHERE id = 5") == [(0,)]
            # BEGIN …; COMMIT in one script leaves nothing open
            call("run_sql", {"connection": DB, "sql": "BEGIN; INSERT INTO u VALUES (6); COMMIT"})
            assert not [c for c in call("get_state")["connections"] if c["name"] == DB][0]["in_transaction"]
            # a BEGIN whose session was disconnected while it ran leaves no transaction behind
            import threading
            slow = "BEGIN; WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM n WHERE x < 30000000) SELECT count(*) FROM n"
            def slow_run():
                try:
                    call("run_sql", {"connection": DB, "sql": slow})
                except AssertionError:
                    pass  # the disconnect interrupts it
            th = threading.Thread(target=slow_run)
            th.start()
            time.sleep(0.3)
            call("disconnect", {"connection": DB})
            th.join(20)
            assert not [c for c in call("get_state")["connections"] if c["name"] == DB][0]["in_transaction"]
            # ROLLBACK TRANSACTION TO a savepoint keeps the transaction open
            call("run_sql", {"connection": DB, "sql": "INSERT INTO lodges(name, river) VALUES ('Sp Lodge', 'Main')"})
            call("run_sql", {"connection": DB, "sql": "SAVEPOINT s"})
            call("run_sql", {"connection": DB, "sql": "ROLLBACK TRANSACTION TO s"})
            assert [c for c in call("get_state")["connections"] if c["name"] == DB][0]["in_transaction"]
            call("rollback", {"connection": DB})
            assert q("SELECT count(*) FROM lodges WHERE name = 'Sp Lodge'") == [(0,)]
            call("set_auto_commit", {"connection": DB, "on": True})
            call("edit_connection", {"connection": DB, "production": True})
            assert "confirm" in call("run_sql", {"connection": DB, "sql": "DELETE FROM missions WHERE id = 6"}, ok=False)
            call("run_sql", {"connection": DB, "sql": "DELETE FROM missions WHERE id = 6", "confirm": True})
            assert q("SELECT count(*) FROM missions") == [(5,)]
            csv = os.path.join(tmp, "prod.csv")
            open(csv, "w").write("name,river\nProd Lodge,Oder\n")
            assert "confirm" in call("import_csv", {"connection": DB, "table": "lodges", "path": csv}, ok=False)
            assert q("SELECT count(*) FROM lodges WHERE name = 'Prod Lodge'") == [(0,)]
            call("edit_connection", {"connection": DB, "production": False, "read_only": True})
            assert "read-only" in call("run_sql", {"connection": DB, "sql": "DELETE FROM missions", "confirm": True}, ok=False)
            assert call("explain", {"sql": "UPDATE missions SET id = id"})["row_count"] >= 1  # plans only
            assert q("SELECT count(*) FROM missions") == [(5,)]
            call("edit_connection", {"connection": DB, "read_only": False})

        def cancel():
            r = {}

            def long_query():
                try:
                    call("run_sql", {"connection": DB, "sql": "WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM n) SELECT count(*) FROM n"}, ok=False)
                    r["ok"] = True
                except Exception as e:  # noqa: BLE001
                    r["err"] = e
            import threading
            th = threading.Thread(target=long_query)
            th.start()
            for _ in range(50):
                time.sleep(0.1)
                try:
                    call("cancel_query")
                    break
                except AssertionError:
                    pass
            th.join(10)
            assert r.get("ok"), r
            # disconnecting stops what runs on that session (it could not be cancelled after)
            r.clear()
            th = threading.Thread(target=long_query)
            th.start()
            time.sleep(0.5)
            call("disconnect", {"connection": DB})
            th.join(10)
            assert r.get("ok"), r

        def io_and_views():
            out = os.path.join(tmp, "beavers.json")
            call("run_sql", {"connection": DB, "sql": "SELECT name, rank FROM beavers"})
            call("export_csv", {"path": out})
            assert json.load(open(out))[0]["name"] == "Marco"
            csv = os.path.join(tmp, "lodges.csv")
            open(csv, "w").write("name,river\nCSV Lodge,Rhine\n")
            assert call("import_csv", {"connection": DB, "table": "lodges", "path": csv})["imported"] == 1
            call("run_sql", {"connection": DB, "sql": "SELECT name, logs_chewed FROM beavers"})
            assert call("chart", {})["value_columns"] == ["logs_chewed"]
            call("chart", {"on": False})
            # a refused column leaves the chart as it was (off)
            assert "numeric" in call("chart", {"on": True, "column": "nope"}, ok=False)
            assert not [t for t in call("get_state")["tabs"] if t["active"]][0]["chart"]
            assert call("record_view", {"on": True, "row": 1})["record"]["name"]
            call("record_view", {"on": False})
            # switching the text view off needs no result to show
            call("run_sql", {"connection": DB, "sql": "CREATE TABLE z(a)"})
            assert call("results_as_text", {"on": False})["text_view"] is False
            call("run_sql", {"connection": DB, "sql": "DROP TABLE z"})
            # a selected row that a search hid: the record the form shows (its last row)
            call("run_sql", {"connection": DB, "sql": "SELECT 1 AS a UNION ALL SELECT 2 UNION ALL SELECT 3"})
            call("record_view", {"on": True, "row": 2})
            call("search_results", {"text": "1"})
            r = call("record_view", {"on": True})
            assert (r["row"], r["record"]) == (0, {"a": "1"}), r
            active = [t for t in call("get_state")["tabs"] if t["active"]][0]
            assert active["result"]["rows"] == 1, active["result"]  # what the grid shows
            call("record_view", {"on": False})
            er = call("open_er_diagram", {"connection": DB})
            assert len(er["relations"]) == 2, er["relations"]
            assert call("generate_sql", {"connection": DB, "table": "lodges", "kind": "select"})["sql"].startswith("SELECT")
            assert len(call("get_history", {"limit": 3})) == 3
            # a failed statement keeps how long it ran
            call("run_sql", {"connection": DB, "sql": "SELECT nope FROM beavers"}, ok=False)
            last = call("get_history", {"limit": 1})[0]
            assert not last["ok"] and last["ms"] > 0, last
            png = call("screenshot")
            assert png is not None

        for name, fn in [("restored session", restored_session), ("navigator + foreign keys", navigator), ("sql editor + scripts + errors", sql_editor),
                         ("parameters, format, explain, completion", params_format_explain), ("table editing, filter, ddl", table_editing),
                         ("manual commit, production + read-only safety", transactions_and_production), ("cancel", cancel),
                         ("export, import, views, er diagram, history", io_and_views)]:
            check(name, fn)
    finally:
        app.terminate()
    print(f"{len(failures)} failed" if failures else "all passed")
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
