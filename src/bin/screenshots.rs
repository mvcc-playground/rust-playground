use screenshots::Screen;
// use std::time::Instant;

fn main() {
    // let start = Instant::now();
    let out_dir = std::path::PathBuf::from(".tmp");
    std::fs::create_dir_all(&out_dir).expect("Error ao criar o out_dr");

    let screens: Vec<Screen> = Screen::all().unwrap();

    for screen in screens {
        // println!("capturer {screen:?}");

        let image = screen.capture().unwrap();
        image
            .save(format!("target/{}.png", screen.display_info.id))
            .expect("Error ao salvar a imagem");

        let path = out_dir.join(format!(
            "screen-{}-{}x{}.png",
            screen.display_info.id,
            image.width(),
            image.height()
        ));

        image.save(&path).expect("Error ao salvar em .tmp");
        println!("Arquivo salvo em {}", path.display())
    }
}
