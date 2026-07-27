use sindon::app::App;
use sindon::core::Color;
use sindon::widgets::tree::WidgetTree;
use sindon::widgets::{Container, TextWidget};

fn main() {
    App::new()
        .title("sindon — hello world")
        .size(800, 600)
        .run(|_scope| {
            let mut tree = WidgetTree::new();
            let root = tree.set_root(Container::column().width(800.0).height(600.0).center());
            tree.add_child(
                root,
                TextWidget::new("Hello, sindon!")
                    .font_size(32.0)
                    .color(Color::rgb(0.2, 0.8, 0.7)),
            );
            tree
        });
}
