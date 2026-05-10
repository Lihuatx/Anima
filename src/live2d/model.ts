import { CubismSetting, Live2DSprite } from 'easy-live2d'
import { Ticker } from 'pixi.js'

export interface CreateLive2DModelSpriteOptions {
  modelEntryUrl: string
}

function toAbsoluteModelEntryUrl(modelEntryUrl: string) {
  return new URL(modelEntryUrl, window.location.href).toString()
}

export async function createLive2DModelSprite(options: CreateLive2DModelSpriteOptions) {
  const modelEntryUrl = toAbsoluteModelEntryUrl(options.modelEntryUrl)
  const response = await fetch(modelEntryUrl)

  if (!response.ok) {
    throw new Error(`Failed to read Live2D model json: ${response.status}`)
  }

  const modelJSON = await response.json()
  const modelSetting = new CubismSetting({ modelJSON })

  modelSetting.redirectPath(({ file }) => new URL(file, modelEntryUrl).toString())

  return new Live2DSprite({
    modelSetting,
    ticker: Ticker.shared,
  })
}
