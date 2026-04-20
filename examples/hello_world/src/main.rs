use shroud::app::App;
use shroud::core::Color;
use shroud::widgets::tree::WidgetTree;
use shroud::widgets::{Container, TextWidget};

fn main() {
    App::new()
        .title("shroud — hello world")
        .size(800, 600)
        .run(|_scope| {
            let mut tree = WidgetTree::new();
            let root = tree.set_root(Container::column().width(800.0).height(600.0).center());
            tree.add_child(
                root,
                TextWidget::new("Hello, shroud!")
                    .font_size(32.0)
                    .color(Color::rgb(0.2, 0.8, 0.7)),
            );
            tree
        });
}
