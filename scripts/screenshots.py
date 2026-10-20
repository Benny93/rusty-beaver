#!/usr/bin/env python3
"""Feature screenshots + animated tour, made by driving the app over MCP.

Starts a fresh app (throwaway connections file), walks through the features, writes
docs/screenshots/NN-name.png and docs/tour.gif (captioned). PostgreSQL / MySQL frames
appear when `make db-up` is running. Needs Pillow for the GIF.

usage: screenshots.py [binary]   (default target/release/rusty-beaver)
"""
import io, json, os, subprocess, sys, tempfile, time

sys.path.insert(0, os.path.dirname(__file__))
import mcp  # noqa: E402  (scripts/mcp.py)

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, "docs", "screenshots")
GIF = os.path.join(ROOT, "docs", "tour.gif")
DEMO = "Beaver Demo (SQLite)"
frames = []  # (png bytes, caption, seconds)


def call(tool, args=None, everything=False):
    mcp.session = None
    mcp.rpc("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "shots", "version": "1"}})
    mcp.rpc("notifications/initialized", notify=True)
    res = mcp.rpc("tools/call", {"name": tool, "arguments": args or {}})
    if res.get("isError"):
        raise RuntimeError(f"{tool}: {res['content'][0]['text']}")
    return res["content"] if everything else res["content"][0]


shots = {}  # name -> png, written only when the whole tour went through


def shot(name=None, caption="", seconds=2.5):
    import base64
    content = call("screenshot", everything=True)
    warnings = [c["text"] for c in content if c.get("type") == "text"]
    if warnings:  # a locked screen does not repaint: every frame would be the same stale one
        sys.exit(f"screenshot: {warnings[0]}\nUnlock the screen and run again; nothing was written.")
    png = base64.b64decode(content[0]["data"])
    if frames and png == frames[-1][0] and name:
        print(f"note: {name} looks exactly like the previous frame")
    if name:
        shots[name] = png
    frames.append((png, caption, seconds))


def wait_for_app(proc):
    for _ in range(100):
        if proc.poll() is not None:
            sys.exit("app exited early")
        try:
            call("screenshot")
            return
        except Exception:
            time.sleep(0.1)
    sys.exit("app did not come up")


def tour(demo):
    shot("01-splash", "Pixel-art splash screen", 2.5)
    time.sleep(2.8)  # let the splash finish on its own

    call("connect", {"connection": DEMO})
    call("describe_table", {"connection": DEMO, "table": "missions"})
    shot("02-navigator", "Database Navigator: tables, views, columns, keys and foreign keys")

    call("open_er_diagram", {"connection": DEMO})
    shot("03-er-diagram", "ER diagram with foreign-key links (drag the boxes)", 3)

    call("open_table", {"connection": DEMO, "table": "beavers"})
    shot(None, "Double-click a table: its data in a fast, sortable grid", 2)
    call("edit_cell", {"row": 2, "column": "rank", "value": "Captain"})
    call("edit_cell", {"row": 4, "column": "joined"})
    call("delete_row", {"row": 7})
    call("add_row", {"values": {"name": "Rookie", "rank": "Recruit", "lodge_id": "4", "logs_chewed": "1"}})
    shot("04-edit-data", "Edit cells, add and delete rows; Save runs it all in one transaction", 3)
    call("revert_changes")
    call("set_table_view", {"view": "columns"})
    shot("05-table-columns", "Columns: types, nullability, primary and foreign keys")
    call("set_table_view", {"view": "ddl"})
    shot(None, "…and the table's DDL", 2)
    call("set_table_view", {"view": "data"})
    call("set_filter", {"filter": "logs_chewed > 5000"})
    shot("06-filter", "Filter rows with a WHERE condition", 2)

    call("new_sql_editor", {"connection": DEMO})
    sql = (
        "-- Who leads the most dam-building missions?\n"
        "SELECT b.name, b.rank, l.name AS lodge,\n"
        "       count(m.id) AS missions, sum(m.dams_built) AS dams\n"
        "FROM beavers b\n"
        "JOIN lodges l ON l.id = b.lodge_id\n"
        "LEFT JOIN missions m ON m.leader_id = b.id\n"
        "GROUP BY b.id\n"
        "ORDER BY dams DESC;"
    )
    lines = sql.split("\n")
    for n in (2, 4, 6, len(lines)):  # "typing" in the SQL editor
        call("set_sql", {"text": "\n".join(lines[:n])})
        shot(None, "SQL editor with highlighting and autocompletion", 0.6)
    call("run_sql", {})
    shot("07-sql-editor", "Cmd/Ctrl+Enter runs the statement at the cursor", 3)
    call("chart", {"column": "dams"})
    shot("08-chart", "Any result as a bar chart", 3)
    call("chart", {"on": False})

    call("run_sql", {"sql": "SELECT * FROM lodges;\nSELECT rank, count(*) AS n FROM beavers GROUP BY rank;\nSELECT codename, status FROM missions;"})
    shot("09-scripts", "Scripts: one result per statement", 2.5)

    call("run_sql", {"sql": "SELECT name, json_object('rank', rank, 'logs', logs_chewed, 'missions',\n  (SELECT json_group_array(codename) FROM missions WHERE leader_id = b.id)) AS profile\nFROM beavers b"})
    call("show_value", {"row": 0, "column": "profile"})
    shot("10-value-panel", "Value panel: long text and JSON, pretty-printed", 2.5)
    call("show_value", {"on": False})

    call("set_sql", {"text": "SELECT name, rank, logs_chewed\nFROM beavers\nWHERE rank = :rank AND logs_chewed >= :min_logs"})
    call("execute_editor")
    shot("11-parameters", "Query parameters are asked for before running", 2.5)
    call("run_sql", {"params": {"rank": "'Private'", "min_logs": "0"}})

    call("format_sql", {"sql": "select l.name, count(*) as beavers from lodges l join beavers b on b.lodge_id = l.id group by l.name having count(*) > 1 order by 2 desc"})
    shot("12-format", "Format SQL (Cmd/Ctrl+Shift+F) and Explain plans", 2)
    call("explain")
    shot(None, "Explain plan of the statement", 2)

    try:  # a typo'd column, on purpose
        call("run_sql", {"sql": "SELECT name, logs_chewed FROM beavers\nORDER BY chewed DESC;"})
    except RuntimeError:
        pass
    shot("13-sql-error", "SQL errors are shown right under the editor")

    call("set_sidebar", {"panel": "history"})
    shot("14-history", "Query history, searchable and kept across restarts", 2.5)
    call("set_sidebar", {"panel": "navigator"})

    call("connection_dialog", {"open": True})
    shot("15-new-connection", "New Connection: SQLite, PostgreSQL or MySQL, with Test Connection")
    call("connection_dialog", {"open": False})

    servers = [
        {"name": "PostgreSQL (docker)", "driver": "postgres", "host": "127.0.0.1", "port": 55432, "database": "postgres", "user": "postgres", "password": "beaver"},
        {"name": "MySQL (docker)", "driver": "mysql", "host": "127.0.0.1", "port": 53306, "database": "beaver", "user": "root", "password": "beaver"},
    ]
    live = []
    for s in servers:
        call("add_connection", s)
        try:
            call("connect", {"connection": s["name"]})
            live.append(s["name"])
        except RuntimeError:
            call("delete_connection", {"connection": s["name"]})
    seed = {  # same demo table on every server, so the navigator has something to show
        "postgres": "CREATE TABLE IF NOT EXISTS rivers(id serial PRIMARY KEY, name text, length_km int, country text);",
        "mysql": "CREATE TABLE IF NOT EXISTS rivers(id int AUTO_INCREMENT PRIMARY KEY, name varchar(40), length_km int, country varchar(40));",
    }
    rows = "INSERT INTO rivers(name, length_km, country) SELECT * FROM (SELECT 'Yukon', 3190, 'Canada/USA' UNION ALL SELECT 'Danube', 2850, 'Europe' UNION ALL SELECT 'Mississippi', 3730, 'USA' UNION ALL SELECT 'Rhine', 1230, 'Europe') AS v{cols} WHERE NOT EXISTS (SELECT 1 FROM rivers);"
    for s in reversed(servers):  # PostgreSQL last: its tab is the one shown
        if s["name"] in live:
            cols = "" if s["driver"] == "mysql" else "(name, length_km, country)"
            call("run_sql", {"connection": s["name"], "sql": seed[s["driver"]] + "\n" + rows.format(cols=cols)})
            call("run_sql", {"connection": s["name"], "sql": "SELECT name, length_km, country, version() AS server FROM rivers ORDER BY length_km DESC;"})
    if live:
        shot("16-multi-database", "One tool for SQLite, PostgreSQL and MySQL")

    call("edit_connection", {"connection": DEMO, "production": True})
    call("open_table", {"connection": DEMO, "table": "lodges"})
    shot("18-production", "Production connections: marked red, changes need confirmation", 2.5)
    call("edit_connection", {"connection": DEMO, "production": False})

    call("set_theme", {"mode": "light"})
    call("open_table", {"connection": DEMO, "table": "missions"})
    shot("17-light-theme", "Dark and light themes")
    call("set_theme", {"mode": "dark"})
    call("open_er_diagram", {"connection": DEMO})
    shot(None, "Every step of this tour was driven by an AI agent over MCP", 3.5)


def make_gif():
    try:
        from PIL import Image, ImageDraw, ImageFont
    except ImportError:
        print("Pillow missing: skipped docs/tour.gif")
        return
    width, bar = 1100, 56
    font = None
    for path in ("/System/Library/Fonts/Supplemental/Arial Bold.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf", "C:/Windows/Fonts/arialbd.ttf"):
        if os.path.exists(path):
            font = ImageFont.truetype(path, 24)
            break
    font = font or ImageFont.load_default()
    out = []
    for png, caption, seconds in frames:
        im = Image.open(io.BytesIO(png)).convert("RGB")
        im = im.resize((width, round(im.height * width / im.width)), Image.LANCZOS)
        canvas = Image.new("RGB", (width, im.height + bar), (26, 20, 16))
        canvas.paste(im, (0, 0))
        d = ImageDraw.Draw(canvas)
        d.rectangle([0, im.height, width, im.height + 3], fill=(224, 112, 42))
        tw = d.textlength(caption, font=font)
        d.text(((width - tw) / 2, im.height + 14), caption, font=font, fill=(242, 196, 106))
        out.append((canvas, seconds))
    imgs = [c.quantize(colors=256, method=Image.Quantize.MEDIANCUT, dither=Image.Dither.NONE) for c, _ in out]
    imgs[0].save(GIF, save_all=True, append_images=imgs[1:], duration=[int(s * 1000) for _, s in out], loop=0, optimize=True)
    print(f"wrote docs/tour.gif ({os.path.getsize(GIF) // 1024} KB, {len(imgs)} frames)")


def main():
    binary = sys.argv[1] if len(sys.argv) > 1 else os.path.join(ROOT, "target", "release", "rusty-beaver")
    os.makedirs(OUT, exist_ok=True)
    tmp = tempfile.mkdtemp()
    cfg = os.path.join(tmp, "connections.json")
    # A fresh copy of the demo database: the tour edits nothing for real, but stays off yours.
    import sqlite3
    demo = os.path.join(tmp, "demo.db")
    con = sqlite3.connect(demo)
    con.executescript(open(os.path.join(ROOT, "src", "demo.sql")).read())
    con.close()
    with open(cfg, "w") as f:
        json.dump([{"name": DEMO, "driver": "sqlite", "database": demo}], f)
    port = os.environ.get("RUSTY_BEAVER_MCP_PORT", "7879")
    mcp.URL = f"http://127.0.0.1:{port}/mcp"
    # HOME: the app shows paths under it (default database file); keep the user's name out.
    env = dict(os.environ, RUSTY_BEAVER_CONFIG=cfg, RUSTY_BEAVER_MCP_PORT=port, RUSTY_BEAVER_BACKGROUND="1", HOME="/Users/beaver", RUSTY_BEAVER_HISTORY=os.path.join(tmp, "history.jsonl"), RUSTY_BEAVER_SESSION=os.path.join(tmp, "session.json"))
    proc = subprocess.Popen([binary], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        wait_for_app(proc)
        tour(demo)
        for name, png in shots.items():
            with open(os.path.join(OUT, f"{name}.png"), "wb") as f:
                f.write(png)
            print("wrote", f"docs/screenshots/{name}.png")
    finally:
        proc.terminate()
    make_gif()


if __name__ == "__main__":
    main()
