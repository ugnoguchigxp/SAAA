import * as THREE from "three";
import { sampleMotion, blendPose } from "./motion.js";
const TAU = Math.PI * 2;
const clamp = THREE.MathUtils.clamp;
const commonDeform = `
uniform float uTime; uniform float uEnergy; uniform float uThinking;
uniform mat4 uHead; uniform vec3 uHeadPivot; uniform vec2 uHeadSpan;
vec3 rigNormal(vec3 n,vec3 p){float w=smoothstep(uHeadSpan.x,uHeadSpan.y,p.y);return normalize(mix(n,(uHead*vec4(n,0.)).xyz,w));}
vec3 deform(vec3 p){
 float weight=smoothstep(uHeadSpan.x,uHeadSpan.y,p.y);
 vec3 turned=(uHead*vec4(p-uHeadPivot,1.)).xyz+uHeadPivot;
 p=mix(p,turned,weight);
 p.x+=sin(p.y*2.1+uTime*.7)*cos(p.z*2.8+uTime*.4)*.004;
 p.z+=sin(p.y*3.+p.x*2.-uTime*.6)*.004;
 return p;
}
`;
const noiseGLSL = `
float hash(vec3 p){p=fract(p*.3183099+vec3(.1,.2,.3));p*=17.0;return fract(p.x*p.y*p.z*(p.x+p.y+p.z));}
float noise(vec3 p){vec3 i=floor(p),f=fract(p);f=f*f*(3.-2.*f);return mix(mix(mix(hash(i),hash(i+vec3(1,0,0)),f.x),mix(hash(i+vec3(0,1,0)),hash(i+vec3(1,1,0)),f.x),f.y),mix(mix(hash(i+vec3(0,0,1)),hash(i+vec3(1,0,1)),f.x),mix(hash(i+vec3(0,1,1)),hash(i+vec3(1,1,1)),f.x),f.y),f.z);}
float fbm(vec3 p){return .55*noise(p)+.28*noise(p*2.07)+.14*noise(p*4.17);}
`;
export function createLightAvatar(host) {
  const renderer = new THREE.WebGLRenderer({
    antialias: true,
    alpha: true,
    premultipliedAlpha: true,
    powerPreference: "low-power",
  });

  const resources = new Set();
  const own = (resource) => {
    resources.add(resource);
    return resource;
  };
  const release = (resource) => {
    if (resources.delete(resource)) resource.dispose();
  };
  let disposed = false;
  function dispose() {
    if (disposed) return;
    disposed = true;
    for (const resource of [...resources]) release(resource);
    renderer.dispose();
    renderer.forceContextLoss();
    renderer.domElement.remove();
  }
  try {
    renderer.info.autoReset = false;
    renderer.debug.onShaderError = (gl, program, vs, fs) => {
      throw new Error("3D shader: " + gl.getShaderInfoLog(vs) + " " + gl.getShaderInfoLog(fs));
    };
    renderer.setPixelRatio(Math.min(devicePixelRatio, 1.25));
    renderer.setClearColor(0x000000, 0);
    renderer.outputColorSpace = THREE.SRGBColorSpace;
    renderer.toneMapping = THREE.ACESFilmicToneMapping;
    renderer.toneMappingExposure = 1.1;
    renderer.domElement.setAttribute("aria-hidden", "true");
    host.append(renderer.domElement);

    const scene = new THREE.Scene();
    const camera = new THREE.PerspectiveCamera(35, 1, 0.1, 100);
    camera.position.set(0, 0.22, 7.7);
    camera.lookAt(0, 0.12, 0);
    const uniforms = {
      uTime: { value: 0 },
      uEnergy: { value: 0.1 },
      uThinking: { value: 0 },
      uFrontFacing: { value: 1 },
      uHead: { value: new THREE.Matrix4() },
      uHeadPivot: { value: new THREE.Vector3() },
      uHeadSpan: { value: new THREE.Vector2() },
    };
    const root = new THREE.Group();
    scene.add(root);
    const palettes = {
      gold: new THREE.Color("#ffdca2"),
      white: new THREE.Color("#fff2d7"),
      blue: new THREE.Color("#b3dbff"),
    };
    let seed = 97245;
    const random = () => {
      seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0;
      return seed / 4294967296;
    };
    const gaussian = () =>
      Math.sqrt(-2 * Math.log(Math.max(0.00001, random()))) * Math.cos(TAU * random());
    const materials = [];
    // A small procedural studio environment supplies soft reflection bands.
    // These cards are rendered into the environment map only, never the stage.
    const studio = new THREE.Scene();
    studio.background = new THREE.Color("#03050a");
    const lightCanvas = document.createElement("canvas");
    lightCanvas.width = 64;
    lightCanvas.height = 128;
    const lightContext = lightCanvas.getContext("2d"),
      gradient = lightContext.createRadialGradient(32, 64, 5, 32, 64, 66);
    gradient.addColorStop(0, "white");
    gradient.addColorStop(0.62, "#aaaaaa");
    gradient.addColorStop(1, "black");
    lightContext.fillStyle = gradient;
    lightContext.fillRect(0, 0, 64, 128);
    const studioTexture = own(new THREE.CanvasTexture(lightCanvas));
    for (const [position, size, color] of [
      [
        [-3, 1, 3],
        [1.1, 5.8],
        [5, 4.3, 3.3],
      ],
      [
        [3, 1.2, 2],
        [1.4, 5.2],
        [2.5, 3.4, 5],
      ],
      [
        [0, 4, -1],
        [4, 1.2],
        [3.8, 3.2, 2.8],
      ],
      [
        [2, -1, -3],
        [0.8, 5],
        [1.9, 1.5, 1.8],
      ],
    ]) {
      const card = new THREE.Mesh(
        own(new THREE.PlaneGeometry(...size)),
        own(
          new THREE.MeshBasicMaterial({
            map: studioTexture,
            color: new THREE.Color(...color),
            side: THREE.DoubleSide,
            toneMapped: false,
          }),
        ),
      );
      card.position.set(...position);
      card.lookAt(0, 0.4, 0);
      studio.add(card);
    }
    const environmentCube = own(
      new THREE.WebGLCubeRenderTarget(128, { type: THREE.HalfFloatType }),
    );
    const cubeCamera = new THREE.CubeCamera(0.1, 20, environmentCube);
    cubeCamera.update(renderer, studio);
    const pmrem = own(new THREE.PMREMGenerator(renderer)),
      environment = own(pmrem.fromCubemap(environmentCube.texture));
    scene.environment = environment.texture;
    release(environmentCube);
    release(pmrem);
    studio.traverse((o) => {
      release(o.geometry);
      release(o.material);
    });
    release(studioTexture);
    const keyLight = new THREE.DirectionalLight("#fff0d7", 2.8);
    keyLight.position.set(-3, 4, 4);
    scene.add(keyLight);
    const fillLight = new THREE.DirectionalLight("#bcd9ff", 1.3);
    fillLight.position.set(3, 1, -2);
    scene.add(fillLight);
    const pearlMaterial = (fold = false, rigged = true) => {
      const m = own(
        new THREE.MeshPhysicalMaterial({
          color: "#fff0da",
          metalness: 0,
          roughness: fold ? 0.12 : 0.095,
          transmission: fold ? 0.87 : 0.97,
          thickness: fold ? 0.035 : 0.15,
          ior: 1.38,
          clearcoat: 1,
          clearcoatRoughness: 0.08,
          iridescence: fold ? 0.45 : 0.65,
          iridescenceIOR: 1.3,
          iridescenceThicknessRange: [180, 410],
          attenuationColor: "#ffeac5",
          attenuationDistance: 2.5,
          envMapIntensity: 1.25,
          emissive: "#9a733b",
          emissiveIntensity: 0.025,
          transparent: true,
          opacity: fold ? 0.48 : 0.46,
          depthWrite: false,
          side: THREE.DoubleSide,
        }),
      );
      m.onBeforeCompile = (shader) => {
        if (!rigged) return;
        Object.assign(shader.uniforms, uniforms);
        shader.vertexShader = commonDeform + shader.vertexShader;
        shader.vertexShader = shader.vertexShader.replace(
          "#include <beginnormal_vertex>",
          "#include <beginnormal_vertex>\nobjectNormal=rigNormal(objectNormal,position);",
        );
        shader.vertexShader = shader.vertexShader.replace(
          "#include <begin_vertex>",
          "#include <begin_vertex>\ntransformed=deform(transformed);",
        );
      };
      m.customProgramCacheKey = () => `saaa-pearl-${fold}-${rigged}`;
      materials.push(m);
      return m;
    };
    const spiritGlass = pearlMaterial(),
      foldGlass = pearlMaterial(true),
      armGlass = pearlMaterial(true, false);
    const baseSpiritCompile = spiritGlass.onBeforeCompile;
    spiritGlass.onBeforeCompile = (shader) => {
      baseSpiritCompile(shader);
      shader.vertexShader = "varying float vTorsoHeight;\n" + shader.vertexShader;
      shader.vertexShader = shader.vertexShader.replace(
        "transformed=deform(transformed);",
        "transformed=deform(transformed);vTorsoHeight=position.y;",
      );
      shader.fragmentShader = "varying float vTorsoHeight;\n" + shader.fragmentShader;
      shader.fragmentShader = shader.fragmentShader.replace(
        "#include <color_fragment>",
        "#include <color_fragment>\ndiffuseColor.a*=.25+.75*smoothstep(-1.3,.35,vTorsoHeight);",
      );
    };
    spiritGlass.customProgramCacheKey = () => "saaa-pearl-torso";
    const surfaceMaterial = (color, opacity = 0.28, soft = false, fade = false) => {
      const m = own(
        new THREE.ShaderMaterial({
          uniforms: {
            ...uniforms,
            uColor: { value: color },
            uOpacity: { value: opacity },
            uSoft: { value: soft ? 1 : 0 },
            uBodyFade: { value: fade ? 1 : 0 },
          },
          transparent: true,
          depthWrite: false,
          side: THREE.DoubleSide,
          blending: THREE.AdditiveBlending,
          vertexShader:
            commonDeform +
            `varying vec3 vN;varying vec3 vV;varying vec3 vP;varying vec2 vUv;void main(){vec3 p=deform(position);vec4 mv=modelViewMatrix*vec4(p,1.);vN=normalize(normalMatrix*rigNormal(normal,position));vV=normalize(-mv.xyz);vP=p;vUv=uv;gl_Position=projectionMatrix*mv;}`,
          fragmentShader:
            noiseGLSL +
            `uniform vec3 uColor;uniform float uOpacity;uniform float uSoft;uniform float uBodyFade;uniform float uTime;uniform float uEnergy;varying vec3 vN;varying vec3 vV;varying vec3 vP;varying vec2 vUv;void main(){float rim=pow(1.-abs(dot(normalize(vN),normalize(vV))),2.3);float w=fbm(vP*3.+vec3(uTime*.12));float veins=pow(max(0.,sin(vUv.x*55.+vUv.y*19.+w*8.-uTime*.35)),mix(28.,85.,uSoft));float a=(.075+rim*(.55-.33*uSoft)+veins*(.16-.13*uSoft)+w*.035)*(uOpacity/.28);a*=mix(1.,.25+.75*smoothstep(-1.3,.35,vP.y),uBodyFade);vec3 c=mix(uColor,vec3(.56,.76,1.),pow(w,4.)*.65);float sheen=pow(max(0.,dot(normalize(vN),normalize(vec3(-.55,.65,1.)))),8.);c=mix(c,mix(uColor,vec3(.76,.83,1.),smoothstep(.48,.66,w)*.48),uSoft);gl_FragColor=vec4(c*(.65+uSoft*.18+rim*(1.-.65*uSoft)+veins*.6+uEnergy*.15+sheen*uSoft*.28),a);}`,
        }),
      );
      materials.push(m);
      return m;
    };
    const lineMaterial = (color, opacity = 0.3, rigged = true) => {
      const m = own(
        new THREE.ShaderMaterial({
          uniforms: { ...uniforms, uColor: { value: color }, uOpacity: { value: opacity } },
          transparent: true,
          depthWrite: false,
          blending: THREE.AdditiveBlending,
          vertexShader:
            (rigged ? commonDeform : "uniform float uTime; vec3 deform(vec3 p){return p;}") +
            `varying float vY;void main(){vY=position.y;gl_Position=projectionMatrix*modelViewMatrix*vec4(deform(position),1.);}`,
          fragmentShader: `uniform vec3 uColor;uniform float uOpacity;uniform float uTime;varying float vY;void main(){float p=.6+.4*sin(vY*3.-uTime*.8);gl_FragColor=vec4(uColor*(.85+p*.4),uOpacity*(.4+p*.6));}`,
        }),
      );
      materials.push(m);
      return m;
    };
    const pointMaterial = (color, size = 1, alpha = 0.65, cloud = false) => {
      const m = own(
        new THREE.ShaderMaterial({
          uniforms: {
            ...uniforms,
            uColor: { value: color },
            uSize: { value: size },
            uAlpha: { value: alpha },
            uPixelRatio: { value: renderer.getPixelRatio() },
          },
          transparent: true,
          depthWrite: false,
          blending: THREE.AdditiveBlending,
          vertexShader:
            commonDeform +
            `attribute float aSize;attribute float aPhase;varying float vAlpha;void main(){vec3 p=deform(position);float phase=aPhase+uTime*(.22+uEnergy*.38);p.x+=sin(phase+p.y*1.3)*(.013+uEnergy*.03);p.y+=sin(phase*1.5)*(.017+uEnergy*.024);vec4 mv=modelViewMatrix*vec4(p,1.);gl_Position=projectionMatrix*mv;gl_PointSize=clamp(aSize*uSize*uPixelRatio*(6.5/-mv.z),.6,${cloud ? "72." : "9."});vAlpha=.5+.5*sin(aPhase+uTime*.8);}`
              .replace("uniform float uTime;", "uniform float uTime;")
              .replace(
                "attribute float aSize;",
                "uniform float uSize;uniform float uPixelRatio;attribute float aSize;",
              ),
          fragmentShader: `uniform vec3 uColor;uniform float uAlpha;uniform float uEnergy;varying float vAlpha;void main(){vec2 p=gl_PointCoord-.5;float d=dot(p,p);if(d>.25)discard;float a=${cloud ? "exp(-d*17.)*pow(1.-d*4.,1.5)" : "exp(-d*29.)"};gl_FragColor=vec4(uColor*(1.+uEnergy*.35),a*uAlpha*(.45+vAlpha*.55));}`,
        }),
      );
      materials.push(m);
      return m;
    };
    function makePoints(list, color, size = 1, alpha = 0.65, cloud = false) {
      const geo = own(new THREE.BufferGeometry());
      geo.setAttribute("position", new THREE.Float32BufferAttribute(list.flat(), 3));
      const sizes = [],
        phases = [];
      for (let i = 0; i < list.length; i++) {
        sizes.push(cloud ? 15 + random() * 35 : 0.55 + random() * 1.8);
        phases.push(random() * TAU);
      }
      geo.setAttribute("aSize", new THREE.Float32BufferAttribute(sizes, 1));
      geo.setAttribute("aPhase", new THREE.Float32BufferAttribute(phases, 1));
      return new THREE.Points(geo, pointMaterial(color, size, alpha, cloud));
    }
    function glow(color, size = 1) {
      const mat = own(
        new THREE.ShaderMaterial({
          uniforms: { uColor: { value: color } },
          transparent: true,
          depthWrite: false,
          blending: THREE.AdditiveBlending,
          side: THREE.DoubleSide,
          vertexShader: `varying vec2 vUv;void main(){vUv=uv;vec4 p=modelViewMatrix*vec4(0.,0.,0.,1.);p.xy+=position.xy*vec2(length(modelMatrix[0].xyz),length(modelMatrix[1].xyz));gl_Position=projectionMatrix*p;}`,
          fragmentShader: `uniform vec3 uColor;varying vec2 vUv;void main(){float r=length(vUv-.5)*2.;float a=exp(-r*r*8.)*.7+exp(-r*r*34.)*.7;gl_FragColor=vec4(uColor,a);}`,
        }),
      );
      materials.push(mat);
      const g = new THREE.Mesh(own(new THREE.PlaneGeometry(1, 1)), mat);
      g.scale.setScalar(size);
      return g;
    }
    const profiles = {
      b: [
        [0, -1.96, 0, 0.008],
        [0.12, -1.48, 0, 0.09],
        [0.26, -0.89, 0, 0.17],
        [0.4, -0.24, 0, 0.22],
        [0.52, 0.25, 0, 0.265],
        [0.6, 0.61, 0, 0.145],
        [0.65, 0.82, 0, 0.21],
        [0.72, 1.08, 0, 0.48],
        [0.8, 1.4, 0, 0.61],
        [0.87, 1.68, 0, 0.53],
        [0.92, 1.81, -0.055, 0.38],
        [0.946, 1.9, -0.17, 0.21],
        [0.965, 1.995, -0.285, 0.1],
        [0.98, 2.095, -0.29, 0.047],
        [0.992, 2.15, -0.22, 0.021],
        [1, 2.14, -0.15, 0.003],
      ],
    };
    function profile(u, kind) {
      const p = profiles[kind];
      let i = 0;
      while (i < p.length - 2 && u > p[i + 1][0]) i++;
      const a = p[i],
        b = p[i + 1],
        t = clamp((u - a[0]) / (b[0] - a[0]), 0, 1);
      const smooth = t * t * (3 - 2 * t);
      if (kind === "b") {
        const prev = p[Math.max(0, i - 1)],
          next = p[Math.min(p.length - 1, i + 2)],
          span = b[0] - a[0];
        const hermite = (column) => {
          const ma = (b[column] - prev[column]) / (b[0] - prev[0]),
            mb = (next[column] - a[column]) / (next[0] - a[0]);
          return (
            (2 * t * t * t - 3 * t * t + 1) * a[column] +
            (t * t * t - 2 * t * t + t) * span * ma +
            (-2 * t * t * t + 3 * t * t) * b[column] +
            (t * t * t - t * t) * span * mb
          );
        };
        return [hermite(1), hermite(2), Math.max(0.001, hermite(3))];
      }
      return [
        THREE.MathUtils.lerp(a[1], b[1], t),
        THREE.MathUtils.lerp(a[2], b[2], smooth),
        THREE.MathUtils.lerp(a[3], b[3], smooth),
      ];
    }
    const shellPoint = (u, phi, kind = "b", scale = 1) => {
      const [y, cx, r] = profile(u, kind),
        fold = 1 + 0.008 * Math.cos(phi * 4);
      return new THREE.Vector3(
        cx + Math.cos(phi) * r * scale * fold,
        y,
        Math.sin(phi) * r * scale * 0.94 * fold,
      );
    };

    function shellGeometry(kind) {
      const positions = [],
        uvs = [],
        indices = [],
        rows = 72,
        cols = 48;
      for (let i = 0; i <= rows; i++)
        for (let j = 0; j <= cols; j++) {
          const t = i / rows,
            u =
              kind === "b" ? (t < 0.72 ? (t / 0.72) * 0.87 : 0.87 + ((t - 0.72) / 0.28) * 0.13) : t,
            p = shellPoint(u, (j / cols) * TAU, kind);
          positions.push(p.x, p.y, p.z);
          uvs.push(j / cols, u);
        }
      for (let i = 0; i < rows; i++)
        for (let j = 0; j < cols; j++) {
          const k = i * (cols + 1) + j;
          indices.push(k, k + cols + 1, k + 1, k + 1, k + cols + 1, k + cols + 2);
        }
      const geo = own(new THREE.BufferGeometry());
      geo.setAttribute("position", new THREE.Float32BufferAttribute(positions, 3));
      geo.setAttribute("uv", new THREE.Float32BufferAttribute(uvs, 2));
      geo.setIndex(indices);
      geo.computeVertexNormals();
      return geo;
    }
    function addReferenceEyes(group, kind) {
      group.userData.eyes = [];
      const eyes = [
        [-0.21, 1.4, 0.55],
        [0.21, 1.4, 0.55],
      ];
      for (const [x, y, z] of eyes) {
        const eye = new THREE.Mesh(
          own(new THREE.CapsuleGeometry(0.022, 0.09, 6, 10)),
          own(new THREE.MeshBasicMaterial({ color: "#fff2b4" })),
        );
        eye.position.set(x, y, z + 0.02);
        eye.rotation.z = x < 0 ? -0.12 : 0.12;
        group.add(eye);
        const g = glow(palettes.gold, 0.16);
        g.position.copy(eye.position);
        group.add(g);
        for (const part of [eye, g]) {
          part.userData.rest = part.position.clone();
          part.userData.eyeRoll = eye.rotation.z;
          part.userData.isGlow = part === g;
        }
        group.userData.eyes.push(eye, g);
      }
    }
    function makeFlame(kind) {
      const group = new THREE.Group();
      const body = new THREE.Mesh(shellGeometry(kind), spiritGlass);
      group.add(body);
      {
        // The body and lower head are symmetric; the crown curls from the upper head itself.
        // Reference imagery informs color, never the projected front silhouette.
        const skin = [],
          headGlow = [];
        for (let n = 0; n < 6000; n += 2) {
          const u = random(),
            phi = random() * TAU,
            p = shellPoint(u, phi, "b", 1 + gaussian() * 0.012);
          const cx = profile(u, "b")[1];
          if (p.y < 0.5) {
            const v = clamp((p.y + 1.96) / 2.55, 0, 1),
              envelope = 0.012 + 0.5 * Math.pow(Math.sin(v * Math.PI), 0.8),
              ratio = envelope / profile(u, "b")[2];
            p.x = cx + (p.x - cx) * ratio;
            p.z *= ratio;
          }
          skin.push([p.x, p.y, p.z], [2 * cx - p.x, p.y, p.z]);
        }
        group.add(makePoints(skin, palettes.gold, 0.6, 0.28));
        for (let n = 0; n < 1000; n += 2) {
          const u = 0.67 + random() * 0.29,
            phi = random() * TAU,
            p = shellPoint(u, phi, "b", 0.98);
          const cx = profile(u, "b")[1];
          headGlow.push([p.x, p.y, p.z], [2 * cx - p.x, p.y, p.z]);
        }
        group.add(makePoints(headGlow, palettes.white, 0.5, 0.14));
        const luminousVeil = new THREE.Mesh(
          shellGeometry("b"),
          surfaceMaterial(palettes.white, 0.15, true, true),
        );
        group.add(luminousVeil);
      }
      const strandCount = kind === "b" ? 28 : 18;
      for (let n = 0; n < strandCount; n++) {
        const points = [];
        const offset = (n / strandCount) * TAU;
        for (let j = 0; j < 120; j++) {
          const u = 0.018 + (j / 119) * 0.967;
          points.push(
            shellPoint(u, offset + u * (n % 2 ? 8 : -7) + Math.sin(u * 7 + n) * 0.45, kind, 1.015),
          );
        }
        group.add(
          new THREE.Line(
            own(new THREE.BufferGeometry()).setFromPoints(points),
            lineMaterial(
              n % 7 === 0 ? palettes.blue : palettes.gold,
              kind === "b" ? (n % 5 === 0 ? 0.06 : 0.028) : n % 5 === 0 ? 0.16 : 0.065,
            ),
          ),
        );
      }
      const particles = [];
      for (let n = 0; n < 2500; n++) {
        const u = random(),
          phi = random() * TAU;
        const p = shellPoint(
          u,
          phi,
          kind,
          random() < 0.82 ? 1 + gaussian() * 0.05 : 1 + random() * 0.35,
        );
        particles.push([p.x, p.y, p.z]);
      }
      group.add(makePoints(particles, palettes.gold, 0.82, 0.5));
      addReferenceEyes(group, kind);
      const core = glow(palettes.gold, 0.65);
      core.position.set(0, 0.06, 0.15);
      group.add(core);
      group.userData.core = core;
      const star = new THREE.Mesh(
        own(new THREE.SphereGeometry(0.056, 16, 10)),
        own(new THREE.MeshBasicMaterial({ color: "#fff4ca" })),
      );
      star.position.copy(core.position);
      group.add(star);
      const inner = [];
      for (let n = 0; n < 500; n++) {
        const r = 0.43 * Math.pow(random(), 0.5),
          a = random() * TAU,
          z = random() * 0.5;
        inner.push([Math.cos(a) * r, 0.06 + Math.sin(a) * r, z - 0.25]);
      }
      group.add(makePoints(inner, palettes.white, 0.7, 0.4));
      if (kind === "b") {
        addArms(group);
        addCoreParticles(group, core.position);
      }
      return group;
    }

    function ribbonGeometry(curve, width, twist = 0, rootWidth = 0) {
      const positions = [],
        uvs = [],
        indices = [],
        rows = 72,
        cols = 8;
      for (let j = 0; j <= rows; j++) {
        const u = j / rows,
          p = curve.getPoint(u),
          t = curve.getTangent(u);
        const side = new THREE.Vector3(-t.y, t.x, Math.sin(u * 8) * 0.45).normalize();
        side.applyAxisAngle(t, twist * Math.sin(u * Math.PI));
        const normal = new THREE.Vector3().crossVectors(t, side).normalize();
        const r = width * (Math.pow(Math.sin(Math.PI * u), 0.7) + rootWidth * Math.pow(1 - u, 3));
        for (let k = 0; k <= cols; k++) {
          const v = (k / cols) * 2 - 1,
            q = p
              .clone()
              .addScaledVector(side, r * v)
              .addScaledVector(normal, Math.cos((v * Math.PI) / 2) * r * 0.42);
          positions.push(q.x, q.y, q.z);
          uvs.push(k / cols, u);
        }
      }
      for (let j = 0; j < rows; j++)
        for (let k = 0; k < cols; k++) {
          const i = j * (cols + 1) + k;
          indices.push(i, i + cols + 1, i + 1, i + 1, i + cols + 1, i + cols + 2);
        }
      const geo = own(new THREE.BufferGeometry());
      geo.setAttribute("position", new THREE.Float32BufferAttribute(positions, 3));
      geo.setAttribute("uv", new THREE.Float32BufferAttribute(uvs, 2));
      geo.setIndex(indices);
      geo.computeVertexNormals();
      return geo;
    }
    const armBendGLSL = `uniform float uArmSwing;uniform float uArmReach;
 vec3 bendArm(vec3 p,float u){float w=smoothstep(0.,1.,u),a=uArmSwing*w,b=uArmReach*w;vec3 q=vec3(cos(a)*p.x-sin(a)*p.y,sin(a)*p.x+cos(a)*p.y,p.z);return vec3(q.x,cos(b)*q.y-sin(b)*q.z,sin(b)*q.y+cos(b)*q.z);}`;
    function addArms(group) {
      const arms = [];
      for (const layer of ["outer", "inner"])
        for (const side of [-1, 1]) {
          const arm = new THREE.Group(),
            isOuter = layer === "outer";
          const points = isOuter
            ? [
                [side * 0.2, 0.33, -0.08],
                [side * 0.64, 0.59, -0.02],
                [side * 0.94, 0.99, 0.03],
                [side * 0.99, 1.35, 0.04],
                [side * 0.88, 1.58, 0.09],
              ]
            : [
                [side * 0.14, 0.18, 0.28],
                [side * 0.43, 0.32, 0.35],
                [side * 0.68, 0.62, 0.39],
                [side * 0.72, 0.92, 0.43],
                [side * 0.59, 1.14, 0.44],
              ];
          const vectors = points.map((p) => new THREE.Vector3(...p)),
            shoulder = vectors[0].clone();
          arm.position.copy(shoulder);
          vectors.forEach((p) => p.sub(shoulder));
          const curve = new THREE.CatmullRomCurve3(vectors),
            bend = { uArmSwing: { value: 0 }, uArmReach: { value: 0 } };
          arm.userData = { side, layer, bend };
          const glass = own(armGlass.clone());
          glass.onBeforeCompile = (shader) => {
            Object.assign(shader.uniforms, bend);
            shader.vertexShader = armBendGLSL + "\n" + shader.vertexShader;
            shader.vertexShader = shader.vertexShader.replace(
              "#include <beginnormal_vertex>",
              "#include <beginnormal_vertex>\nobjectNormal=bendArm(objectNormal,uv.y);",
            );
            shader.vertexShader = shader.vertexShader.replace(
              "#include <begin_vertex>",
              "#include <begin_vertex>\ntransformed=bendArm(transformed,uv.y);",
            );
          };
          glass.customProgramCacheKey = () => "saaa-flexible-arm";
          arm.add(new THREE.Mesh(ribbonGeometry(curve, isOuter ? 0.175 : 0.11, 0, 0.3), glass));
          for (let k = 0; k < 3; k++) {
            const points2 = [],
              weights = [];
            for (let j = 0; j < 80; j++) {
              const u = j / 79,
                p = curve.getPoint(u);
              p.z += Math.sin(u * 7 + k) * 0.016;
              p.x += Math.sin(u * Math.PI) * (k - 1) * 0.011;
              points2.push(p);
              weights.push(u);
            }
            const geometry = own(new THREE.BufferGeometry()).setFromPoints(points2);
            geometry.setAttribute("aU", new THREE.Float32BufferAttribute(weights, 1));
            const material = lineMaterial(palettes.gold, 0.18, false);
            Object.assign(material.uniforms, bend);
            material.vertexShader =
              armBendGLSL +
              "attribute float aU;varying float vY;void main(){vY=position.y;gl_Position=projectionMatrix*modelViewMatrix*vec4(bendArm(position,aU),1.);}";
            arm.add(new THREE.Line(geometry, material));
          }
          const grains = [],
            grainU = [];
          for (let n = 0; n < 240; n++) {
            const u = random(),
              p = curve.getPoint(u),
              t = curve.getTangent(u),
              sideVector = new THREE.Vector3(-t.y, t.x, 0).normalize(),
              r = (isOuter ? 0.175 : 0.11) * Math.pow(Math.sin(u * Math.PI), 0.7);
            p.addScaledVector(sideVector, (random() * 2 - 1) * r * 0.8);
            p.z += gaussian() * 0.018;
            grains.push([p.x, p.y, p.z]);
            grainU.push(u);
          }
          const dust = makePoints(grains, palettes.gold, 0.65, 0.4);
          dust.geometry.setAttribute("aU", new THREE.Float32BufferAttribute(grainU, 1));
          Object.assign(dust.material.uniforms, bend);
          dust.material.vertexShader =
            armBendGLSL +
            "\nattribute float aU;\n" +
            dust.material.vertexShader.replace(
              "vec3 p=deform(position);",
              "vec3 p=bendArm(position,aU);",
            );
          arm.add(dust);
          const tip = glow(palettes.gold, isOuter ? 0.135 : 0.105);
          tip.position.copy(vectors.at(-1));
          arm.add(tip);
          arm.userData.tip = tip;
          arm.userData.tipRest = tip.position.clone();
          arms.push(arm);
          group.add(arm);
        }
      group.userData.arms = arms;
      // Open, overlapping waist membranes curl around the luminous center and meet at a narrow hem.
      for (const side of [-1, 1])
        for (const layer of [0, 1]) {
          const points =
            layer === 0
              ? [
                  [side * 0.18, 0.48, 0.12],
                  [side * 0.39, 0.08, 0.29],
                  [side * 0.56, -0.48, 0.15],
                  [side * 0.45, -1.03, 0.25],
                  [side * 0.2, -1.6, 0.03],
                  [0, -1.96, 0],
                ]
              : [
                  [side * 0.14, 0.26, 0.31],
                  [side * 0.43, -0.12, 0.44],
                  [side * 0.25, -0.54, 0.43],
                  [-side * 0.12, -0.96, 0.25],
                  [-side * 0.15, -1.35, 0.18],
                  [0, -1.96, 0],
                ];
          const curve = new THREE.CatmullRomCurve3(points.map((p) => new THREE.Vector3(...p))),
            geo = ribbonGeometry(
              curve,
              layer === 0 ? 0.31 : 0.19,
              side * (layer === 0 ? 1.0 : -1.1),
            );
          group.add(new THREE.Mesh(geo, foldGlass));
          group.add(new THREE.Mesh(geo, surfaceMaterial(palettes.gold, 0.07, true)));
          for (let k = 0; k < 3; k++) {
            const line = [];
            for (let j = 0; j < 100; j++) {
              const u = j / 99,
                p = curve.getPoint(u);
              p.x += side * Math.sin(u * Math.PI) * (k - 1) * 0.035;
              line.push(p);
            }
            group.add(
              new THREE.Line(
                own(new THREE.BufferGeometry()).setFromPoints(line),
                lineMaterial(k === 0 ? palettes.blue : palettes.gold, 0.085),
              ),
            );
          }
        }
    }
    function addCoreParticles(group, center) {
      const list = [];
      for (let n = 0; n < 2000; n++) {
        const angle = random() * TAU,
          r = (0.11 + Math.pow(random(), 0.65) * 0.28) * (1 + 0.1 * Math.sin(angle * 5 + (n % 3))),
          lane = n % 3;
        const p = new THREE.Vector3(
          Math.cos(angle) * r,
          Math.sin(angle) * r * 0.68,
          gaussian() * 0.018,
        );
        p.applyAxisAngle(new THREE.Vector3(1, 0, 0), [0.45, -0.65, 1.2][lane]);
        p.applyAxisAngle(new THREE.Vector3(0, 0, 1), lane * 0.75);
        p.add(center);
        list.push([p.x, p.y, p.z]);
      }
      const swarm = makePoints(list, palettes.gold, 0.95, 1);
      swarm.material.uniforms.uOrbitCenter = { value: center.clone() };
      swarm.material.vertexShader = swarm.material.vertexShader.replace(
        "vec3 p=deform(position);",
        "vec3 q=position-uOrbitCenter;float a=uTime*.23*(.7+fract(aPhase)*.6),c=cos(a),s=sin(a);q.xz=mat2(c,-s,s,c)*q.xz;vec3 p=deform(q+uOrbitCenter);",
      );
      swarm.material.vertexShader = "uniform vec3 uOrbitCenter;" + swarm.material.vertexShader;
      group.add(swarm);
      group.userData.coreParticles = 2000;
      for (let lane = 0; lane < 3; lane++) {
        const points = [];
        for (let j = 0; j <= 120; j++) {
          const a = (j / 120) * TAU,
            r = (0.21 + lane * 0.048) * (1 + 0.11 * Math.sin(a * 3 + lane)),
            p = new THREE.Vector3(Math.cos(a) * r, Math.sin(a) * r * 0.66, 0);
          p.applyAxisAngle(new THREE.Vector3(1, 0, 0), [0.45, -0.65, 1.2][lane]);
          p.applyAxisAngle(new THREE.Vector3(0, 0, 1), lane * 0.75);
          p.add(center);
          points.push(p);
        }
        group.add(
          new THREE.Line(
            own(new THREE.BufferGeometry()).setFromPoints(points),
            lineMaterial(palettes.gold, 0.1),
          ),
        );
      }
    }
    const models = { b: makeFlame("b") };
    for (const [id, m] of Object.entries(models)) {
      m.visible = id === "b";
      root.add(m);
    }
    // A soft plane beneath the avatar anchors its 3D volume without introducing another illustration.
    const floor = new THREE.Mesh(
      own(new THREE.PlaneGeometry(5, 5)),
      own(
        new THREE.ShaderMaterial({
          uniforms: { uColor: { value: palettes.gold } },
          transparent: true,
          depthWrite: false,
          blending: THREE.AdditiveBlending,
          side: THREE.DoubleSide,
          vertexShader: `varying vec2 vUv;void main(){vUv=uv;gl_Position=projectionMatrix*modelViewMatrix*vec4(position,1.);}`,
          fragmentShader: `uniform vec3 uColor;varying vec2 vUv;void main(){float r=length(vUv-.5)*2.;float a=exp(-r*r*18.)*.13+pow(max(0.,1.-abs(r-.43)*20.),2.)*.045;gl_FragColor=vec4(uColor,a);}`,
        }),
      ),
    );
    floor.rotation.x = -Math.PI / 2;
    floor.position.y = -1.96;
    scene.add(floor);
    // Lightweight bloom: local half-resolution buffers, two blur passes, no addon dependency.
    const base = own(new THREE.WebGLRenderTarget(1, 1, { type: THREE.HalfFloatType })),
      ping = own(new THREE.WebGLRenderTarget(1, 1)),
      pong = own(new THREE.WebGLRenderTarget(1, 1));
    const quadScene = new THREE.Scene(),
      quadCamera = new THREE.OrthographicCamera(-1, 1, 1, -1, 0, 1),
      quad = new THREE.Mesh(own(new THREE.PlaneGeometry(2, 2)));
    quadScene.add(quad);
    const blur = own(
      new THREE.ShaderMaterial({
        uniforms: {
          uTexture: { value: null },
          uDirection: { value: new THREE.Vector2(1, 0) },
          uThreshold: { value: 0 },
        },
        vertexShader: `varying vec2 vUv;void main(){vUv=uv;gl_Position=vec4(position.xy,0.,1.);}`,
        fragmentShader: `uniform sampler2D uTexture;uniform vec2 uDirection;uniform float uThreshold;varying vec2 vUv;vec3 read(vec2 uv){vec3 c=texture2D(uTexture,uv).rgb;return max(vec3(0.),c-vec3(uThreshold));}void main(){vec3 c=read(vUv)*.227027;c+=read(vUv+uDirection*1.384615)*.316216;c+=read(vUv-uDirection*1.384615)*.316216;c+=read(vUv+uDirection*3.230769)*.07027;c+=read(vUv-uDirection*3.230769)*.07027;gl_FragColor=vec4(c,1.);}`,
      }),
    );
    const combine = own(
      new THREE.ShaderMaterial({
        uniforms: { uBase: { value: base.texture }, uGlow: { value: pong.texture } },
        vertexShader: `varying vec2 vUv;void main(){vUv=uv;gl_Position=vec4(position.xy,0.,1.);}`,
        fragmentShader:
          `uniform sampler2D uBase;uniform sampler2D uGlow;varying vec2 vUv;void main(){vec3 c=texture2D(uBase,vUv).rgb+texture2D(uGlow,vUv).rgb*.65;gl_FragColor=vec4(c,1.);#include <tonemapping_fragment>\n#include <colorspace_fragment>\n gl_FragColor.a=clamp(max(texture2D(uBase,vUv).a,max(gl_FragColor.r,max(gl_FragColor.g,gl_FragColor.b))),0.,1.);}`.replace(
            ";#include",
            ";\n#include",
          ),
      }),
    );
    let current = "b",
      yaw = 0,
      pitch = 0,
      w = 1,
      h = 1;
    function resize() {
      w = Math.max(1, host.clientWidth);
      h = Math.max(1, host.clientHeight);
      renderer.setSize(w, h, false);
      camera.aspect = w / h;
      camera.position.z = camera.aspect < 0.65 ? 8.6 : 7.7;
      camera.updateProjectionMatrix();
      const d = renderer.getPixelRatio();
      base.setSize(Math.round(w * d), Math.round(h * d));
      ping.setSize(Math.round((w * d) / 2), Math.round((h * d) / 2));
      pong.setSize(Math.round((w * d) / 2), Math.round((h * d) / 2));
    }
    let activeMode = null,
      previousPose = sampleMotion("neutral", 0),
      acting = previousPose;
    const headEuler = new THREE.Euler(),
      headRotation = new THREE.Quaternion(),
      headCamera = new THREE.Vector3(),
      headNormal = new THREE.Vector3(),
      eyeQuaternion = new THREE.Quaternion();
    function render(time = 0, mode = "neutral", elapsed = 0) {
      renderer.info.reset();
      uniforms.uTime.value = time;
      if (activeMode !== mode) {
        previousPose = { ...acting };
        activeMode = mode;
      }
      const phraseTime = Math.max(0, elapsed);
      acting = blendPose(
        previousPose,
        sampleMotion(mode, phraseTime),
        Math.min(1, phraseTime / 0.7),
      );
      const p = acting;
      uniforms.uEnergy.value = p.energy;
      uniforms.uThinking.value = mode === "thinking" ? 1 : 0;
      root.position.set(0, p.lift + Math.sin(time * 0.65) * 0.008, p.forward);
      root.rotation.set(pitch + p.bx, yaw + p.by + (current === "b" ? 0.34 : 0), p.bz);
      root.scale.setScalar(1);
      const m = models[current],
        pivot = uniforms.uHeadPivot.value;
      pivot.set(0, 0.68, 0);
      uniforms.uHeadSpan.value.set(0.57, 1.18);
      headEuler.set(p.hx, p.hy, p.hz);
      headRotation.setFromEuler(headEuler);
      uniforms.uHead.value.makeRotationFromQuaternion(headRotation);
      if (m.userData.arms)
        m.userData.arms.forEach((arm) => {
          const d = arm.userData,
            swing =
              d.layer === "outer"
                ? d.side < 0
                  ? p.left
                  : p.right
                : d.side < 0
                  ? p.leftInner
                  : p.rightInner,
            reach =
              d.layer === "outer" ? p.hx * 0.06 : (d.side < 0 ? p.rightInner : -p.leftInner) * 0.35;
          d.bend.uArmSwing.value = swing;
          d.bend.uArmReach.value = reach;
          const q = d.tipRest.clone(),
            c = Math.cos(swing),
            s = Math.sin(swing),
            x = c * q.x - s * q.y,
            y = s * q.x + c * q.y;
          d.tip.position.set(
            x,
            Math.cos(reach) * y - Math.sin(reach) * q.z,
            Math.sin(reach) * y + Math.cos(reach) * q.z,
          );
        });
      m.userData.core.scale.setScalar((current === "b" ? 0.6 : 0.65) * (1 + p.energy * 0.08));
      root.updateMatrixWorld(true);
      const localCamera = headCamera
        .copy(camera.position)
        .applyMatrix4(new THREE.Matrix4().copy(root.matrixWorld).invert());
      headNormal.set(0, 0, 1).applyQuaternion(headRotation);
      uniforms.uFrontFacing.value = headNormal.dot(localCamera.clone().sub(pivot).normalize());
      if (m.userData.eyes)
        m.userData.eyes.forEach((eye) => {
          const rest = eye.userData.rest,
            weight = THREE.MathUtils.smoothstep(
              rest.y,
              uniforms.uHeadSpan.value.x,
              uniforms.uHeadSpan.value.y,
            );
          const turned = rest.clone().sub(pivot).applyQuaternion(headRotation).add(pivot);
          eye.position.copy(rest).lerp(turned, weight);
          eye.position.add(new THREE.Vector3(p.gx, p.gy, 0).applyQuaternion(headRotation));
          eyeQuaternion.setFromAxisAngle(new THREE.Vector3(0, 0, 1), eye.userData.eyeRoll);
          eye.quaternion.copy(headRotation).multiply(eyeQuaternion);
          const open = Math.max(0.025, p.open);
          eye.scale.set(
            eye.userData.isGlow ? 0.125 : 1,
            eye.userData.isGlow ? open * 0.128 : open,
            eye.userData.isGlow ? 0.16 : 1,
          );
          eye.visible = uniforms.uFrontFacing.value > 0.06;
        });
      renderer.setRenderTarget(base);
      renderer.render(scene, camera);
      quad.material = blur;
      blur.uniforms.uTexture.value = base.texture;
      blur.uniforms.uDirection.value.set(2.2 / ping.width, 0);
      blur.uniforms.uThreshold.value = 0.24;
      renderer.setRenderTarget(ping);
      renderer.render(quadScene, quadCamera);
      blur.uniforms.uTexture.value = ping.texture;
      blur.uniforms.uDirection.value.set(0, 2.2 / pong.height);
      blur.uniforms.uThreshold.value = 0;
      renderer.setRenderTarget(pong);
      renderer.render(quadScene, quadCamera);
      quad.material = combine;
      renderer.setRenderTarget(null);
      renderer.render(quadScene, quadCamera);
    }
    resize();
    render();
    return {
      canvas: renderer.domElement,
      resize,
      render,
      beginMotion() {
        activeMode = null;
      },
      stats() {
        return {
          time: Number(uniforms.uTime.value.toFixed(3)),
          armCount: models[current].userData.arms?.length || 0,
          armSwings:
            models[current].userData.arms
              ?.map((a) => Number(a.userData.bend.uArmSwing.value.toFixed(3)))
              .join(",") || "",
          coreParticles: models[current].userData.coreParticles || 0,
          motion: activeMode,
          headTilt: Number(acting.hz.toFixed(3)),
          headNod: Number(acting.hx.toFixed(3)),
          headTurn: Number(acting.hy.toFixed(3)),
          eyeOpen: Number(acting.open.toFixed(3)),
          bodyScale: root.scale.y,
          eyesVisible: uniforms.uFrontFacing.value > 0.06,
          frontFacing: Number(uniforms.uFrontFacing.value.toFixed(3)),
          renderer: "Three.js WebGL",
          revision: THREE.REVISION,
          model: current,
          geometries: renderer.info.memory.geometries,
          drawCalls: renderer.info.render.calls,
          points: renderer.info.render.points,
          triangles: renderer.info.render.triangles,
          yaw: Number(yaw.toFixed(3)),
          pitch: Number(pitch.toFixed(3)),
        };
      },
      dispose,
    };
  } catch (error) {
    dispose();
    throw error;
  }
}
