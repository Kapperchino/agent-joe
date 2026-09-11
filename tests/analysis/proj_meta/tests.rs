use super::*;

fn rpath(path: &str) -> RPath {
    RPath {
        inner: path.to_string(),
    }
}

fn range(start: u32, end: u32) -> Range {
    Range { start, end }
}

#[test]
fn display_groups_symbols_by_file() {
    let proj_meta = ProjMeta {
        enums: vec![],
        structs: vec![StructMeta {
            rpath: rpath("src/app/src/utils/draw_line.rs"),
            full_range: range(332, 360),
            name: "LineStyle".to_string(),
            docs: None,
            fields: vec![
                FieldMeta {
                    rpath: rpath("src/app/src/utils/draw_line.rs"),
                    full_range: range(333, 333),
                    name: "thickness".to_string(),
                    docs: None,
                },
                FieldMeta {
                    rpath: rpath("src/app/src/utils/draw_line.rs"),
                    full_range: range(334, 334),
                    name: "color".to_string(),
                    docs: None,
                },
                FieldMeta {
                    rpath: rpath("src/app/src/utils/draw_line.rs"),
                    full_range: range(335, 335),
                    name: "pattern".to_string(),
                    docs: None,
                },
            ],
            functions: vec![],
        }],
        functions: vec![
            FunctionMeta {
                rpath: rpath("src/app/src/utils/draw_line.rs"),
                full_range: range(1, 239),
                name: "draw_line".to_string(),
                docs: Some("Draws a line segment into the terminal buffer.".to_string()),
                discription: Some("fn draw_line(...) -> Result<()>".to_string()),
            },
            FunctionMeta {
                rpath: rpath("src/app/src/utils/draw_line.rs"),
                full_range: range(240, 331),
                name: "clip_line".to_string(),
                docs: None,
                discription: Some("fn clip_line(...) -> Option<Line>".to_string()),
            },
            FunctionMeta {
                rpath: rpath("src/app/src/utils/draw_table.rs"),
                full_range: range(1, 220),
                name: "draw_table".to_string(),
                docs: None,
                discription: Some("fn draw_table(...) -> Result<()>".to_string()),
            },
        ],
        type_alias: vec![],
        traits: vec![],
        files: FnvHashMap::default(),
    };

    assert_eq!(
        proj_meta.to_string(),
        concat!(
            "# Repo Symbols\n",
            "\n",
            "## src/app/src/utils/draw_line.rs\n",
            "\n",
            "- fn draw_line(...) -> Result<()> [1-239]\n",
            "  docs: Draws a line segment into the terminal buffer.\n",
            "\n",
            "- fn clip_line(...) -> Option<Line> [240-331]\n",
            "\n",
            "- struct LineStyle [332-360]\n",
            "  fields: thickness, color, pattern\n",
            "\n",
            "## src/app/src/utils/draw_table.rs\n",
            "\n",
            "- fn draw_table(...) -> Result<()> [1-220]\n",
        )
    );
}
