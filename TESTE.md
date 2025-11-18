> Para capturar áudio “interno” no macOS você precisa de um dispositivo de loopback, porque o Core Audio não expõe diretamente o mix do sistema:

- Instalar um driver virtual como BlackHole (gratuito), Loopback (pago) ou o antigo Soundflower. Eles criam um dispositivo de entrada que recebe o som vindo da saída.
- Configurar o macOS: em “Configuração de Áudio MIDI” crie um dispositivo agregado/saída múltipla que use o driver virtual, depois defina esse agregado como saída padrão. Agora tudo o que tocar
  no sistema vai para o driver virtual.
- Ajustar seu programa: com o dispositivo configurado, basta selecionar no cpal o input que representa o driver virtual (por exemplo, usando host.input_devices() e filtrando pelo nome). O
  restante do código que salva em WAV continua igual.
- Verificar permissões: macOS ainda pedirá acesso a microfone para o app, porque do ponto de vista do sistema trata-se de uma “entrada”.
- Alternativas: se não quiser mexer em drivers, dá para usar o ffmpeg -f avfoundation -i ":0" capturando direto pela API do macOS, mas o passo do driver loopback continua sendo necessário.

Naturalmente cada driver tem instruções próprias de instalação e ajuste de latência, então vale seguir a documentação deles.
