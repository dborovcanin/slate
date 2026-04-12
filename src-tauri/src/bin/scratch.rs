#[allow(dead_code)]
#[path = "../terminal/render.rs"]
mod render;

fn main() {
    let mut ctx = render::RenderContext::new();
    let text = "  # Heading\n  - List\n**bold** and *italic*\n***bold italic***\n _italic_ \n __bold__ ";
    for line in text.lines() {
        let out = ctx.render_line(line, 80, None, &[]);
        println!("{}", out);
    }
}
